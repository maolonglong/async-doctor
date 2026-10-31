//! Encode live [`ProjectFacts`] into per-file cache blobs + import keys.

use std::collections::BTreeMap;

use async_doctor_core::{
    CallKind, CalleeRef, EntryLabel, FunctionFact, FunctionId, LiteralArg, ProjectFacts,
};

use crate::cache::{CACHE_SCHEMA_VERSION, CachedFollowTarget, TOOL_VERSION};

use super::{
    CachedFileEdge, CachedFileFacts, CachedFileFn, CachedFileUnresolvedImport, CachedProjectRef,
};
use crate::extract::ctx::DefKey;

/// Map first-party import heads from `source` onto content fingerprints.
pub fn import_fps_for_source(
    source: &str,
    file_module: &str,
    module_fps: &BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for head in scrape_import_modules(source, file_module) {
        if let Some(fp) = lookup_module_fp(module_fps, &head) {
            out.insert(head, fp);
        }
    }
    out
}

/// True when every recorded import fingerprint still matches `module_fps`.
pub fn import_fps_still_valid(
    cached: &BTreeMap<String, String>,
    module_fps: &BTreeMap<String, String>,
) -> bool {
    cached.iter().all(|(mod_name, fp)| {
        lookup_module_fp(module_fps, mod_name)
            .as_ref()
            .is_some_and(|cur| cur == fp)
    })
}

fn lookup_module_fp(module_fps: &BTreeMap<String, String>, name: &str) -> Option<String> {
    if let Some(fp) = module_fps.get(name) {
        return Some(fp.clone());
    }
    let mut cur = name;
    while let Some((parent, _)) = cur.rsplit_once('.') {
        if let Some(fp) = module_fps.get(parent) {
            return Some(fp.clone());
        }
        cur = parent;
    }
    None
}

fn scrape_import_modules(source: &str, file_module: &str) -> Vec<String> {
    let mut out = Vec::new();
    for raw in source.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(rest) = line.strip_prefix("import ") {
            for part in rest.split(',') {
                let full = part.split_whitespace().next().unwrap_or("").to_string();
                if !full.is_empty() {
                    out.push(full);
                }
            }
        } else if let Some(rest) = line.strip_prefix("from ") {
            let mut bits = rest.split_whitespace();
            let Some(mod_part) = bits.next() else {
                continue;
            };
            if mod_part.starts_with('.') {
                if let Some(resolved) = resolve_relative_import(file_module, mod_part) {
                    out.push(resolved);
                }
            } else if !mod_part.is_empty() {
                out.push(mod_part.to_string());
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

fn resolve_relative_import(file_module: &str, rel: &str) -> Option<String> {
    let dots = rel.chars().take_while(|c| *c == '.').count();
    let rest = &rel[dots..];
    let mut parts: Vec<&str> = file_module.split('.').filter(|s| !s.is_empty()).collect();
    if !file_module.is_empty() {
        let _ = parts.pop();
    }
    if dots > 1 {
        let up = dots - 1;
        if up > parts.len() {
            return None;
        }
        parts.truncate(parts.len() - up);
    }
    if !rest.is_empty() {
        parts.extend(rest.split('.').filter(|s| !s.is_empty()));
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("."))
    }
}

fn label_str(l: EntryLabel) -> &'static str {
    match l {
        EntryLabel::Route => "route",
        EntryLabel::Lifecycle => "lifecycle",
        EntryLabel::Task => "task",
        EntryLabel::Other => "other",
    }
}

fn call_kind_to_str(k: CallKind) -> &'static str {
    match k {
        CallKind::Call => "Call",
        CallKind::WithEnter => "WithEnter",
        CallKind::WithExit => "WithExit",
        CallKind::ForIter => "ForIter",
        CallKind::PropertyGet => "PropertyGet",
        CallKind::GeneratorConsume => "GeneratorConsume",
    }
}

/// Build a per-file cache blob from live facts for one relative path.
#[allow(clippy::too_many_arguments)]
pub fn file_facts_from_project(
    facts: &ProjectFacts,
    rel: &str,
    content_fp: &str,
    venv_fp: &str,
    import_fps: BTreeMap<String, String>,
    unresolved_third_party_modules: Vec<String>,
    def_map: &std::collections::HashMap<DefKey, FunctionId>,
    follow_targets: &std::collections::HashMap<String, crate::sitepkgs::FollowTarget>,
    task_targets: &std::collections::HashSet<FunctionId>,
    property_defs: &std::collections::HashSet<DefKey>,
) -> CachedFileFacts {
    let file_fns: Vec<&FunctionFact> = facts
        .functions
        .iter()
        .filter(|f| f.file == rel && !f.is_third_party)
        .collect();
    let id_set: std::collections::HashSet<FunctionId> = file_fns.iter().map(|f| f.id).collect();

    // Full id → DefKey so Project callees in *other* files keep file+name_start.
    let mut def_by_id: BTreeMap<u32, &DefKey> = BTreeMap::new();
    let mut name_start_by_id: BTreeMap<u32, u32> = BTreeMap::new();
    for (key, id) in def_map {
        def_by_id.insert(id.0, key);
        if key.file == rel {
            name_start_by_id.insert(id.0, key.name_start);
        }
    }

    let functions: Vec<CachedFileFn> = file_fns
        .iter()
        .filter_map(|f| {
            let name_start = name_start_by_id.get(&f.id.0).copied()?;
            Some(CachedFileFn {
                name_start,
                line: f.line,
                module: f.module.clone(),
                qualname: f.qualname.clone(),
                is_async: f.is_async,
                is_generator: f.is_generator,
                label: label_str(f.label).to_string(),
                is_sync_route: f.is_sync_route,
                param_names: f.param_names.clone(),
                invokes_params: f.invokes_params.clone(),
                is_offload_wrapper: f.is_offload_wrapper,
                async_twins: f.async_twins.clone(),
            })
        })
        .collect();

    let project_ref = |id: FunctionId| -> Option<CachedProjectRef> {
        let f = facts.functions.get(id.index())?;
        if f.is_third_party {
            return None;
        }
        let key = def_by_id.get(&id.0)?;
        Some(CachedProjectRef {
            module: f.module.clone(),
            qualname: f.qualname.clone(),
            file: key.file.clone(),
            name_start: key.name_start,
        })
    };

    let edges: Vec<CachedFileEdge> = facts
        .edges
        .iter()
        .filter(|e| id_set.contains(&e.caller))
        .filter_map(|e| {
            let caller_name_start = *name_start_by_id.get(&e.caller.0)?;
            let (
                callee_kind,
                callee_qualname,
                callee_module,
                callee_file,
                callee_name_start,
                offload_kind,
                param_invoke_index,
            ) = match &e.callee {
                CalleeRef::External { qualname } => (
                    "external".to_string(),
                    qualname.clone(),
                    None,
                    None,
                    None,
                    None,
                    None,
                ),
                CalleeRef::Offload(k) => (
                    "offload".to_string(),
                    k.as_str().to_string(),
                    None,
                    None,
                    None,
                    Some(k.as_str().to_string()),
                    None,
                ),
                CalleeRef::Unresolved { reason } => (
                    "unresolved".to_string(),
                    reason.clone(),
                    None,
                    None,
                    None,
                    None,
                    None,
                ),
                CalleeRef::ParamInvoke { index } => (
                    "param_invoke".to_string(),
                    format!("param:{index}"),
                    None,
                    None,
                    None,
                    None,
                    Some(*index),
                ),
                CalleeRef::Project(id) => {
                    let f = &facts.functions[id.index()];
                    if f.is_third_party {
                        (
                            "external".to_string(),
                            f.qualname.clone(),
                            None,
                            None,
                            None,
                            None,
                            None,
                        )
                    } else {
                        (
                            "project".to_string(),
                            f.qualname.clone(),
                            Some(f.module.clone()),
                            Some(f.file.clone()),
                            def_by_id.get(&id.0).map(|k| k.name_start),
                            None,
                            None,
                        )
                    }
                }
            };
            Some(CachedFileEdge {
                caller_name_start,
                callee_kind,
                callee_qualname,
                callee_module,
                callee_file,
                callee_name_start,
                offload_kind,
                param_invoke_index,
                is_offload_wrapper: e.is_offload_wrapper,
                kind: call_kind_to_str(e.kind).to_string(),
                file: e.file.clone(),
                line: e.line,
                text: e.text.clone(),
                conditions: e
                    .conditions
                    .iter()
                    .map(|c| c.as_str().to_string())
                    .collect(),
                occurrence: e.occurrence,
                positional_literals: e
                    .positional_literals
                    .iter()
                    .map(|o| {
                        o.map(|l| match l {
                            LiteralArg::True => "true".to_string(),
                            LiteralArg::False => "false".to_string(),
                            LiteralArg::None => "none".to_string(),
                        })
                    })
                    .collect(),
                positional_callable_refs: e
                    .positional_callables
                    .iter()
                    .map(|o| o.and_then(project_ref))
                    .collect(),
                keyword_callable_refs: e
                    .keyword_callables
                    .iter()
                    .filter_map(|(k, id)| project_ref(*id).map(|r| (k.clone(), r)))
                    .collect(),
                param_guard_index: e.param_guard.map(|g| g.index),
                param_guard_inverted: e.param_guard.map(|g| g.inverted),
                is_awaited: e.is_awaited,
                callee_is_coroutine: e.callee_is_coroutine,
                is_eager_offload_arg: e.is_eager_offload_arg,
                context_from_caller: e.context_from_caller.and_then(project_ref),
            })
        })
        .collect();

    let mut follow = BTreeMap::new();
    for e in &edges {
        if matches!(e.callee_kind.as_str(), "external" | "third_party")
            && let Some(t) = follow_targets.get(&e.callee_qualname)
        {
            follow.insert(
                e.callee_qualname.clone(),
                CachedFollowTarget {
                    qualname: t.qualname.clone(),
                    path: t.path.as_str().to_string(),
                    name_start: t.name_start,
                    package_name: t.package_name.clone(),
                    package_version: t.package_version.clone(),
                    package_fingerprint: t.package_fingerprint.clone(),
                },
            );
        }
    }

    CachedFileFacts {
        schema_version: CACHE_SCHEMA_VERSION,
        tool_version: TOOL_VERSION.to_string(),
        rel: rel.to_string(),
        content_fingerprint: content_fp.to_string(),
        venv_fingerprint: venv_fp.to_string(),
        import_fps,
        functions,
        edges,
        unresolved_imports: facts
            .unresolved_imports
            .iter()
            .filter(|u| u.file == rel)
            .map(|u| CachedFileUnresolvedImport {
                line: u.line,
                module: u.module.clone(),
                text: u.text.clone(),
            })
            .collect(),
        unresolved_third_party_modules,
        follow_targets: follow,
        task_target_name_starts: task_targets
            .iter()
            .filter_map(|id| name_start_by_id.get(&id.0).copied())
            .collect(),
        property_def_name_starts: property_defs
            .iter()
            .filter(|k| k.file == rel)
            .map(|k| k.name_start)
            .collect(),
    }
}
