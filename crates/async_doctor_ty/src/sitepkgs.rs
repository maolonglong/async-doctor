//! Site-packages helpers: dist-info metadata and third-party follow targets.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use async_doctor_core::{
    CallEdge, CallKind, CalleeRef, ConditionTag, EntryLabel, FunctionFact, FunctionId, OffloadKind,
    ProjectFacts,
};
use async_doctor_rules::BuiltinSinkTable;
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
            // End of header fields.
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

    let entries = fs::read_dir(&site_packages).ok()?;
    let top_norm = normalize_dist_name(&top);
    for entry in entries.flatten() {
        let fname = entry.file_name();
        let fname = fname.to_string_lossy();
        if !fname.ends_with(".dist-info") {
            continue;
        }
        let stem = fname.trim_end_matches(".dist-info");
        // `{name}-{version}.dist-info`
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

/// Materialize cached package functions that are requested into `facts`.
///
/// Returns the set of qualnames that were materialized from cache.
pub fn materialize_cached_functions(
    facts: &mut ProjectFacts,
    package: &str,
    version: &str,
    wanted: &[String],
    qual_to_id: &mut HashMap<String, FunctionId>,
) -> Vec<String> {
    let Some(cache) = load_package_cache(package, version) else {
        return Vec::new();
    };
    let sinks = BuiltinSinkTable::new();
    let mut loaded = Vec::new();
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
            file: format!("site-packages/{package}/{}.py", cf.module.replace('.', "/")),
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
        for edge in &cf.edges {
            let callee = cached_callee_to_ref(edge, &sinks);
            facts.edges.push(CallEdge {
                caller: id,
                callee,
                file: format!("site-packages/{package}"),
                line: edge.line,
                text: edge.text.clone(),
                kind: CallKind::Call,
                conditions: vec![ConditionTag::Unconditional, ConditionTag::AllPaths],
                occurrence: 0,
                is_offload_wrapper: edge.is_offload_wrapper,
                positional_literals: Vec::new(),
                positional_callables: Vec::new(),
                param_guard: None,
            });
        }
        loaded.push(q.clone());
    }
    loaded
}

fn cached_callee_to_ref(edge: &CachedEdge, _sinks: &BuiltinSinkTable) -> CalleeRef {
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
        _ => CalleeRef::External {
            qualname: edge.callee_qualname.clone(),
        },
    }
}

/// Update (or create) the on-disk cache entry for one analyzed third-party function.
pub fn upsert_cache_function(
    package: &str,
    version: &str,
    func: &FunctionFact,
    edges: &[CallEdge],
) {
    let mut cache = load_package_cache(package, version).unwrap_or_else(|| PackageCache {
        package: package.to_string(),
        version: version.to_string(),
        functions: HashMap::new(),
    });
    let key = func.full_qualname();
    let cached_edges: Vec<CachedEdge> = edges
        .iter()
        .filter(|e| e.caller == func.id)
        .map(|e| {
            let (callee_kind, callee_qualname, offload_kind) = match &e.callee {
                CalleeRef::External { qualname } => {
                    ("external".to_string(), qualname.clone(), None)
                }
                CalleeRef::Offload(k) => (
                    "offload".to_string(),
                    k.as_str().to_string(),
                    Some(k.as_str().to_string()),
                ),
                CalleeRef::Unresolved { reason } => {
                    ("unresolved".to_string(), reason.clone(), None)
                }
                CalleeRef::Project(id) => {
                    // Store by the project's full qualname when available — caller fills later.
                    ("external".to_string(), format!("project:{}", id.0), None)
                }
                CalleeRef::ParamInvoke { index } => {
                    ("unresolved".to_string(), format!("param:{index}"), None)
                }
            };
            CachedEdge {
                callee_qualname,
                callee_kind,
                offload_kind,
                is_offload_wrapper: e.is_offload_wrapper,
                line: e.line,
                text: e.text.clone(),
            }
        })
        .collect();
    cache.functions.insert(
        key,
        CachedFunction {
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
    let _ = save_package_cache(&cache);
}
