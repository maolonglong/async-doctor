//! Site-packages helpers: dist-info metadata, follow targets, and package cache I/O.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use async_doctor_core::{
    CallEdge, CallKind, CalleeRef, ConditionTag, EntryLabel, FunctionFact, FunctionId, LiteralArg,
    OffloadKind, ParamGuard, ProjectFacts,
};
use ruff_db::system::SystemPathBuf;

use crate::cache::{
    CachedEdge, CachedFunction, PackageCache, load_package_cache, path_str_contains_site_packages,
    save_package_cache,
};

/// A third-party `.py` definition we may extract on demand.
#[derive(Debug, Clone)]
pub struct FollowTarget {
    pub qualname: String,
    pub path: SystemPathBuf,
    pub name_start: u32,
    pub package_name: Option<String>,
    pub package_version: Option<String>,
}

/// Parse `Name:` / `Version:` from a dist-info `METADATA` file.
pub fn parse_dist_info_metadata(text: &str) -> Option<(String, String)> {
    let mut name = None;
    let mut version = None;
    for line in text.lines() {
        if line.is_empty() {
            if name.is_some() {
                break;
            }
            continue;
        }
        if let Some(rest) = line.strip_prefix("Name:") {
            name = Some(rest.trim().to_string());
        } else if let Some(rest) = line.strip_prefix("Version:") {
            version = Some(rest.trim().to_string());
        }
        if name.is_some() && version.is_some() {
            break;
        }
    }
    Some((name?, version?))
}

/// Locate neighboring `*.dist-info/METADATA` for a file under site-packages.
pub fn package_meta_for_path(py_path: &Path) -> Option<(String, String)> {
    let s = py_path.to_string_lossy().replace('\\', "/");
    let idx = s.find("/site-packages/")?;
    let site_packages = PathBuf::from(&s[..idx + "/site-packages".len()]);
    let rest = &s[idx + "/site-packages/".len()..];
    let top = rest.split('/').next()?.to_string();
    if top.is_empty() {
        return None;
    }

    let mut entries: Vec<_> = fs::read_dir(&site_packages).ok()?.flatten().collect();
    // Deterministic order for identical meta discovery.
    entries.sort_by_key(|e| e.file_name());
    let top_norm = normalize_dist_name(&top);
    for entry in entries {
        let fname = entry.file_name();
        let fname = fname.to_string_lossy();
        if !fname.ends_with(".dist-info") {
            continue;
        }
        let stem = fname.trim_end_matches(".dist-info");
        let dist_name = stem.split('-').next().unwrap_or(stem);
        if normalize_dist_name(dist_name) != top_norm {
            continue;
        }
        let meta_path = entry.path().join("METADATA");
        let text = fs::read_to_string(meta_path).ok()?;
        if let Some(meta) = parse_dist_info_metadata(&text) {
            return Some(meta);
        }
    }
    None
}

fn normalize_dist_name(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c == '_' || c == '.' {
                '-'
            } else {
                c.to_ascii_lowercase()
            }
        })
        .collect()
}

/// True when a system path string is third-party Python source under site-packages.
pub fn is_followable_site_packages_py(path_str: &str) -> bool {
    let s = path_str.replace('\\', "/");
    path_str_contains_site_packages(&s) && s.ends_with(".py") && !s.ends_with(".pyi")
}

/// In-memory batch of package caches to flush once at the end of a follow pass.
#[derive(Default)]
pub struct PackageCacheBatch {
    /// (package, version) → cache being built this run.
    caches: BTreeMap<(String, String), PackageCache>,
}

impl PackageCacheBatch {
    pub fn new() -> Self {
        Self::default()
    }

    fn cache_mut(&mut self, package: &str, version: &str) -> &mut PackageCache {
        let key = (package.to_string(), version.to_string());
        self.caches.entry(key).or_insert_with(|| {
            load_package_cache(package, version)
                .unwrap_or_else(|| PackageCache::new(package, version))
        })
    }

    /// Record one analyzed third-party function under its External qualname key.
    pub fn upsert_function(
        &mut self,
        package: &str,
        version: &str,
        external_key: &str,
        func: &FunctionFact,
        edges: &[CallEdge],
        id_to_external_key: &HashMap<FunctionId, String>,
    ) {
        let cache = self.cache_mut(package, version);
        let cached_edges: Vec<CachedEdge> = edges
            .iter()
            .filter(|e| e.caller == func.id)
            .map(|e| edge_to_cached(e, id_to_external_key))
            .collect();
        cache.functions.insert(
            external_key.to_string(),
            CachedFunction {
                key: external_key.to_string(),
                qualname: func.qualname.clone(),
                module: func.module.clone(),
                line: func.line,
                is_async: func.is_async,
                is_generator: func.is_generator,
                param_names: func.param_names.clone(),
                invokes_params: func.invokes_params.clone(),
                is_offload_wrapper: func.is_offload_wrapper,
                edges: cached_edges,
            },
        );
    }

    /// Atomically write every dirty package cache once.
    pub fn flush(&self) {
        for cache in self.caches.values() {
            let _ = save_package_cache(cache);
        }
    }
}

fn edge_to_cached(e: &CallEdge, id_to_external_key: &HashMap<FunctionId, String>) -> CachedEdge {
    let (callee_kind, callee_qualname, offload_kind, param_invoke_index) = match &e.callee {
        CalleeRef::External { qualname } => ("external".to_string(), qualname.clone(), None, None),
        CalleeRef::Offload(k) => (
            "offload".to_string(),
            k.as_str().to_string(),
            Some(k.as_str().to_string()),
            None,
        ),
        CalleeRef::Unresolved { reason } => ("unresolved".to_string(), reason.clone(), None, None),
        CalleeRef::Project(id) => {
            // Prefer stable External key for third-party Project callees.
            let key = id_to_external_key
                .get(id)
                .cloned()
                .unwrap_or_else(|| format!("unresolved:project:{}", id.0));
            let kind = if id_to_external_key.contains_key(id) {
                "third_party"
            } else {
                "unresolved"
            };
            (kind.to_string(), key, None, None)
        }
        CalleeRef::ParamInvoke { index } => (
            "param_invoke".to_string(),
            format!("param:{index}"),
            None,
            Some(*index),
        ),
    };
    let positional_literals = e
        .positional_literals
        .iter()
        .map(|lit| {
            lit.map(|l| match l {
                LiteralArg::True => "true".to_string(),
                LiteralArg::False => "false".to_string(),
                LiteralArg::None => "none".to_string(),
            })
        })
        .collect();
    let positional_callable_quals = e
        .positional_callables
        .iter()
        .map(|opt| opt.and_then(|fid| id_to_external_key.get(&fid).cloned()))
        .collect();
    CachedEdge {
        callee_qualname,
        callee_kind,
        offload_kind,
        param_invoke_index,
        is_offload_wrapper: e.is_offload_wrapper,
        kind: call_kind_str(e.kind).to_string(),
        file: e.file.clone(),
        line: e.line,
        text: e.text.clone(),
        conditions: e
            .conditions
            .iter()
            .map(|c| c.as_str().to_string())
            .collect(),
        occurrence: e.occurrence,
        positional_literals,
        positional_callable_quals,
        param_guard_index: e.param_guard.map(|g| g.index),
        param_guard_inverted: e.param_guard.map(|g| g.inverted),
    }
}

fn call_kind_str(kind: CallKind) -> &'static str {
    match kind {
        CallKind::Call => "Call",
        CallKind::WithEnter => "WithEnter",
        CallKind::WithExit => "WithExit",
        CallKind::ForIter => "ForIter",
        CallKind::PropertyGet => "PropertyGet",
        CallKind::GeneratorConsume => "GeneratorConsume",
    }
}

fn parse_call_kind(s: &str) -> CallKind {
    match s {
        "WithEnter" => CallKind::WithEnter,
        "WithExit" => CallKind::WithExit,
        "ForIter" => CallKind::ForIter,
        "PropertyGet" => CallKind::PropertyGet,
        "GeneratorConsume" => CallKind::GeneratorConsume,
        _ => CallKind::Call,
    }
}

fn parse_condition(s: &str) -> Option<ConditionTag> {
    match s {
        "unconditional" => Some(ConditionTag::Unconditional),
        "all-paths" => Some(ConditionTag::AllPaths),
        "loop" => Some(ConditionTag::Loop),
        "async-branch" => Some(ConditionTag::AsyncBranch),
        "cache_miss_dependent" => Some(ConditionTag::CacheMissDependent),
        "sync_entry" => Some(ConditionTag::SyncEntry),
        _ => None,
    }
}

fn parse_literal(s: &str) -> Option<LiteralArg> {
    match s {
        "true" => Some(LiteralArg::True),
        "false" => Some(LiteralArg::False),
        "none" => Some(LiteralArg::None),
        _ => None,
    }
}

/// Materialize cached package functions that are requested into `facts`.
///
/// Keys in `wanted` must be External qualnames (same as cache keys).
/// Returns the set of keys that were materialized from cache.
pub fn materialize_cached_functions(
    facts: &mut ProjectFacts,
    package: &str,
    version: &str,
    wanted: &[String],
    qual_to_id: &mut BTreeMap<String, FunctionId>,
    id_to_external_key: &mut HashMap<FunctionId, String>,
) -> Vec<String> {
    let Some(cache) = load_package_cache(package, version) else {
        return Vec::new();
    };
    let mut loaded = Vec::new();
    // First pass: create FunctionFacts for wanted keys present in cache.
    let mut pending_edges: Vec<(FunctionId, Vec<CachedEdge>)> = Vec::new();
    for q in wanted {
        if qual_to_id.contains_key(q) {
            continue;
        }
        let Some(cf) = cache.functions.get(q) else {
            continue;
        };
        let id = FunctionId(facts.functions.len() as u32);
        facts.functions.push(FunctionFact {
            id,
            file: format!("site-packages/{}.py", cf.module.replace('.', "/")),
            line: cf.line,
            module: cf.module.clone(),
            qualname: cf.qualname.clone(),
            is_async: cf.is_async,
            is_generator: cf.is_generator,
            label: EntryLabel::Other,
            is_sync_route: false,
            async_twins: Vec::new(),
            param_names: cf.param_names.clone(),
            is_third_party: true,
            package_name: Some(package.to_string()),
            package_version: Some(version.to_string()),
            invokes_params: cf.invokes_params.clone(),
            is_offload_wrapper: cf.is_offload_wrapper,
        });
        qual_to_id.insert(q.clone(), id);
        id_to_external_key.insert(id, q.clone());
        pending_edges.push((id, cf.edges.clone()));
        loaded.push(q.clone());
    }

    // Transitive closure: materialize every cached callee referenced by edges
    // (third_party / external / positional callables) so warm runs do not need
    // follow_targets for nested site-packages hops.
    let mut frontier: Vec<String> = Vec::new();
    for (_, edges) in &pending_edges {
        collect_cacheable_callees(edges, &mut frontier);
    }
    frontier.sort();
    frontier.dedup();
    let mut fi = 0;
    while fi < frontier.len() {
        let q = frontier[fi].clone();
        fi += 1;
        if qual_to_id.contains_key(&q) {
            continue;
        }
        let Some(cf) = cache.functions.get(&q) else {
            continue;
        };
        let id = FunctionId(facts.functions.len() as u32);
        facts.functions.push(FunctionFact {
            id,
            file: format!("site-packages/{}.py", cf.module.replace('.', "/")),
            line: cf.line,
            module: cf.module.clone(),
            qualname: cf.qualname.clone(),
            is_async: cf.is_async,
            is_generator: cf.is_generator,
            label: EntryLabel::Other,
            is_sync_route: false,
            async_twins: Vec::new(),
            param_names: cf.param_names.clone(),
            is_third_party: true,
            package_name: Some(package.to_string()),
            package_version: Some(version.to_string()),
            invokes_params: cf.invokes_params.clone(),
            is_offload_wrapper: cf.is_offload_wrapper,
        });
        qual_to_id.insert(q.clone(), id);
        id_to_external_key.insert(id, q.clone());
        pending_edges.push((id, cf.edges.clone()));
        loaded.push(q.clone());
        let mut more = Vec::new();
        collect_cacheable_callees(&cf.edges, &mut more);
        for m in more {
            if !qual_to_id.contains_key(&m) && !frontier.contains(&m) {
                frontier.push(m);
            }
        }
    }

    // Second pass: emit edges with full summaries, resolving third_party → Project.
    for (caller, edges) in pending_edges {
        for edge in edges {
            let callee = cached_callee_to_ref(&edge, qual_to_id);
            let positional_literals = edge
                .positional_literals
                .iter()
                .map(|o| o.as_deref().and_then(parse_literal))
                .collect();
            let positional_callables = edge
                .positional_callable_quals
                .iter()
                .map(|o| o.as_ref().and_then(|q| qual_to_id.get(q).copied()))
                .collect();
            let param_guard = match (edge.param_guard_index, edge.param_guard_inverted) {
                (Some(index), Some(inverted)) => Some(ParamGuard { index, inverted }),
                _ => None,
            };
            let conditions: Vec<ConditionTag> = edge
                .conditions
                .iter()
                .filter_map(|s| parse_condition(s))
                .collect();
            let edge_file = if edge.file.is_empty() {
                let mod_path = facts.functions[caller.index()].module.replace('.', "/");
                format!("site-packages/{mod_path}.py")
            } else {
                edge.file.clone()
            };
            facts.edges.push(CallEdge {
                caller,
                callee,
                file: edge_file,
                line: edge.line,
                text: edge.text.clone(),
                kind: parse_call_kind(&edge.kind),
                conditions,
                occurrence: edge.occurrence,
                is_offload_wrapper: edge.is_offload_wrapper,
                positional_literals,
                positional_callables,
                param_guard,
            });
        }
    }
    loaded.sort();
    loaded
}

fn collect_cacheable_callees(edges: &[CachedEdge], out: &mut Vec<String>) {
    for e in edges {
        if matches!(e.callee_kind.as_str(), "third_party" | "external") {
            out.push(e.callee_qualname.clone());
        }
        for cq in e.positional_callable_quals.iter().flatten() {
            out.push(cq.clone());
        }
    }
}

fn cached_callee_to_ref(edge: &CachedEdge, qual_to_id: &BTreeMap<String, FunctionId>) -> CalleeRef {
    match edge.callee_kind.as_str() {
        "offload" => {
            let kind = match edge.offload_kind.as_deref() {
                Some("asyncio.to_thread") => OffloadKind::AsyncioToThread,
                Some("loop.run_in_executor") => OffloadKind::RunInExecutor,
                Some("anyio.to_thread.run_sync") => OffloadKind::AnyioToThread,
                Some("starlette.concurrency.run_in_threadpool") => OffloadKind::StarletteThreadpool,
                Some("concurrent.futures.ThreadPoolExecutor") => OffloadKind::ThreadPoolExecutor,
                _ => OffloadKind::AsyncioToThread,
            };
            CalleeRef::Offload(kind)
        }
        "unresolved" => CalleeRef::Unresolved {
            reason: edge.callee_qualname.clone(),
        },
        "param_invoke" => CalleeRef::ParamInvoke {
            index: edge.param_invoke_index.unwrap_or(0),
        },
        "third_party" | "external" => {
            if let Some(id) = qual_to_id.get(&edge.callee_qualname) {
                CalleeRef::Project(*id)
            } else {
                // Leave as External so a later follow wave / sink table can
                // resolve it; never invent project:N.
                CalleeRef::External {
                    qualname: edge.callee_qualname.clone(),
                }
            }
        }
        _ => CalleeRef::External {
            qualname: edge.callee_qualname.clone(),
        },
    }
}
