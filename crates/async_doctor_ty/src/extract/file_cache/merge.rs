//! Merge cached file facts into live [`ProjectFacts`].

use async_doctor_core::{
    CallEdge, CallKind, CalleeRef, ConditionTag, EntryLabel, FunctionFact, FunctionId, LiteralArg,
    OffloadKind, ParamGuard, ProjectFacts, UnresolvedImport,
};
use ruff_db::system::SystemPathBuf;

use crate::sitepkgs::FollowTarget;

use super::{CachedFileFacts, CachedProjectRef};
use crate::extract::ctx::DefKey;

type DefMap = std::collections::HashMap<DefKey, FunctionId>;
type QualMap = std::collections::HashMap<(String, String), FunctionId>;

fn parse_label(s: &str) -> EntryLabel {
    match s {
        "route" => EntryLabel::Route,
        "lifecycle" => EntryLabel::Lifecycle,
        "task" => EntryLabel::Task,
        _ => EntryLabel::Other,
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
        "no_running_loop_guard" => Some(ConditionTag::NoRunningLoopGuard),
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

fn edge_from_cached(
    facts: &ProjectFacts,
    def_map: &DefMap,
    qual_map: &QualMap,
    cf: &CachedFileFacts,
    e: &super::CachedFileEdge,
) -> Option<CallEdge> {
    let &caller = def_map.get(&DefKey {
        file: cf.rel.clone(),
        name_start: e.caller_name_start,
    })?;
    let resolve = |r: &CachedProjectRef| -> Option<FunctionId> {
        resolve_project_ref(&facts.functions, def_map, qual_map, r)
    };
    let callee = match e.callee_kind.as_str() {
        "project" => {
            let module = e.callee_module.clone().unwrap_or_default();
            match resolve_project_callee(
                &facts.functions,
                def_map,
                qual_map,
                e.callee_file.as_deref(),
                e.callee_name_start,
                &module,
                &e.callee_qualname,
            ) {
                Some(id) => CalleeRef::Project(id),
                None => CalleeRef::Unresolved {
                    reason: format!("project:{module}.{}", e.callee_qualname),
                },
            }
        }
        "offload" => {
            let kind = match e.offload_kind.as_deref() {
                Some("asyncio.to_thread") => OffloadKind::AsyncioToThread,
                Some("loop.run_in_executor") => OffloadKind::RunInExecutor,
                Some("anyio.to_thread.run_sync") => OffloadKind::AnyioToThread,
                Some("starlette.concurrency.run_in_threadpool") => OffloadKind::StarletteThreadpool,
                Some("concurrent.futures.ThreadPoolExecutor") => OffloadKind::ThreadPoolExecutor,
                Some("asgiref.sync.sync_to_async") => OffloadKind::AsgirefSyncToAsync,
                _ => OffloadKind::AsyncioToThread,
            };
            CalleeRef::Offload(kind)
        }
        "unresolved" => CalleeRef::Unresolved {
            reason: e.callee_qualname.clone(),
        },
        "param_invoke" => CalleeRef::ParamInvoke {
            index: e.param_invoke_index.unwrap_or(0),
        },
        _ => CalleeRef::External {
            qualname: e.callee_qualname.clone(),
        },
    };
    let positional_callables: Vec<_> = e
        .positional_callable_refs
        .iter()
        .map(|o| o.as_ref().and_then(resolve))
        .collect();
    let keyword_callables: Vec<_> = e
        .keyword_callable_refs
        .iter()
        .filter_map(|(k, r)| resolve(r).map(|id| (k.clone(), id)))
        .collect();
    let context_from_caller = e.context_from_caller.as_ref().and_then(resolve);
    Some(CallEdge {
        caller,
        callee,
        file: if e.file.is_empty() {
            cf.rel.clone()
        } else {
            e.file.clone()
        },
        line: e.line,
        text: e.text.clone(),
        kind: parse_call_kind(&e.kind),
        conditions: e
            .conditions
            .iter()
            .filter_map(|s| parse_condition(s))
            .collect(),
        occurrence: e.occurrence,
        is_offload_wrapper: e.is_offload_wrapper,
        positional_literals: e
            .positional_literals
            .iter()
            .map(|o| o.as_deref().and_then(parse_literal))
            .collect(),
        positional_callables,
        positional_externals: e.positional_externals.clone(),
        positional_constructed_receivers: e.positional_constructed_receivers.clone(),
        keyword_callables,
        param_guard: match (e.param_guard_index, e.param_guard_inverted) {
            (Some(index), Some(inverted)) => Some(ParamGuard { index, inverted }),
            _ => None,
        },
        is_awaited: e.is_awaited,
        callee_is_coroutine: e.callee_is_coroutine,
        is_eager_offload_arg: e.is_eager_offload_arg,
        context_from_caller,
        constructed_receiver: e.constructed_receiver.clone(),
    })
}

/// Merge cached file facts (sorted by `rel`) into empty [`ProjectFacts`].
///
/// Assigns FunctionIds in file order; remaps Project callees via qual_map.
pub fn merge_cached_files(
    cached: &[CachedFileFacts],
    follow_targets: &mut std::collections::HashMap<String, FollowTarget>,
) -> (ProjectFacts, DefMap, QualMap) {
    let mut facts = ProjectFacts {
        complete: true,
        ..ProjectFacts::default()
    };
    let mut def_map = std::collections::HashMap::new();
    let mut qual_map = std::collections::HashMap::new();

    for cf in cached {
        for f in &cf.functions {
            let id = FunctionId(facts.functions.len() as u32);
            def_map.insert(
                DefKey {
                    file: cf.rel.clone(),
                    name_start: f.name_start,
                },
                id,
            );
            qual_map.insert((f.module.clone(), f.qualname.clone()), id);
            facts.functions.push(FunctionFact {
                id,
                file: cf.rel.clone(),
                line: f.line,
                module: f.module.clone(),
                qualname: f.qualname.clone(),
                is_async: f.is_async,
                is_generator: f.is_generator,
                label: parse_label(&f.label),
                is_sync_route: f.is_sync_route,
                async_twins: f.async_twins.clone(),
                param_names: f.param_names.clone(),
                is_third_party: false,
                package_name: None,
                package_version: None,
                invokes_params: f.invokes_params.clone(),
                is_offload_wrapper: f.is_offload_wrapper,
            });
        }
        for u in &cf.unresolved_imports {
            facts.unresolved_imports.push(UnresolvedImport {
                file: cf.rel.clone(),
                line: u.line,
                module: u.module.clone(),
                text: u.text.clone(),
            });
        }
        for m in &cf.unresolved_third_party_modules {
            facts.unresolved_third_party_modules.push(m.clone());
        }
        for (q, t) in &cf.follow_targets {
            follow_targets
                .entry(q.clone())
                .or_insert_with(|| FollowTarget {
                    qualname: t.qualname.clone(),
                    path: SystemPathBuf::from(t.path.as_str()),
                    name_start: t.name_start,
                    package_name: t.package_name.clone(),
                    package_version: t.package_version.clone(),
                    package_fingerprint: t.package_fingerprint.clone(),
                });
        }
    }

    for cf in cached {
        for e in &cf.edges {
            if let Some(edge) = edge_from_cached(&facts, &def_map, &qual_map, cf, e) {
                // Skip synthetic project→Unresolved remaps; callers should
                // promote via [`cached_project_callees_resolvable`] first.
                if matches!(
                    &edge.callee,
                    CalleeRef::Unresolved { reason } if reason.starts_with("project:")
                ) {
                    continue;
                }
                facts.edges.push(edge);
            }
        }
        for &ns in &cf.task_target_name_starts {
            if let Some(&id) = def_map.get(&DefKey {
                file: cf.rel.clone(),
                name_start: ns,
            }) {
                let f = &mut facts.functions[id.index()];
                if f.label == EntryLabel::Other {
                    f.label = EntryLabel::Task;
                }
            }
        }
    }

    (facts, def_map, qual_map)
}

/// True when every cached Project callee (and callable ref) still remaps.
///
/// Used before treating a content-hit as final: a failed remap means the file
/// must be re-extracted (soundness over keeping a synthetic Unresolved edge).
pub fn cached_project_callees_resolvable(
    facts: &ProjectFacts,
    def_map: &DefMap,
    qual_map: &QualMap,
    cf: &CachedFileFacts,
) -> bool {
    let resolve_ok = |module: &str, qualname: &str, file: Option<&str>, ns: Option<u32>| {
        resolve_project_callee(
            &facts.functions,
            def_map,
            qual_map,
            file,
            ns,
            module,
            qualname,
        )
        .is_some()
    };
    for e in &cf.edges {
        if e.callee_kind == "project"
            && !resolve_ok(
                e.callee_module.as_deref().unwrap_or(""),
                &e.callee_qualname,
                e.callee_file.as_deref(),
                e.callee_name_start,
            )
        {
            return false;
        }
        for r in e.positional_callable_refs.iter().flatten() {
            if !resolve_ok(&r.module, &r.qualname, Some(&r.file), Some(r.name_start)) {
                return false;
            }
        }
        for (_, r) in &e.keyword_callable_refs {
            if !resolve_ok(&r.module, &r.qualname, Some(&r.file), Some(r.name_start)) {
                return false;
            }
        }
        if let Some(r) = &e.context_from_caller
            && !resolve_ok(&r.module, &r.qualname, Some(&r.file), Some(r.name_start))
        {
            return false;
        }
    }
    true
}

/// Append edges/unresolved from hit caches without re-adding functions
/// (functions already restored in pass-1 / prior merge).
pub fn append_cached_edges_only(
    facts: &mut ProjectFacts,
    cached: &[CachedFileFacts],
    def_map: &DefMap,
    qual_map: &QualMap,
) {
    for cf in cached {
        for e in &cf.edges {
            if let Some(edge) = edge_from_cached(facts, def_map, qual_map, cf, e) {
                // Skip synthetic project→Unresolved remaps; caller should have
                // promoted such files via [`cached_project_callees_resolvable`].
                if matches!(
                    &edge.callee,
                    CalleeRef::Unresolved { reason } if reason.starts_with("project:")
                ) {
                    continue;
                }
                facts.edges.push(edge);
            }
        }
        for u in &cf.unresolved_imports {
            let already = facts
                .unresolved_imports
                .iter()
                .any(|x| x.file == cf.rel && x.module == u.module && x.line == u.line);
            if !already {
                facts.unresolved_imports.push(UnresolvedImport {
                    file: cf.rel.clone(),
                    line: u.line,
                    module: u.module.clone(),
                    text: u.text.clone(),
                });
            }
        }
    }
}

/// Remap a cached project callee: DefKey hit only when module+qualname still
/// match (offset shifts must not re-point at a different function). Fall back
/// to `(module, qualname)`; otherwise `None` (caller should re-extract).
pub(crate) fn resolve_project_callee(
    functions: &[FunctionFact],
    def_map: &DefMap,
    qual_map: &QualMap,
    callee_file: Option<&str>,
    callee_name_start: Option<u32>,
    module: &str,
    qualname: &str,
) -> Option<FunctionId> {
    if let (Some(file), Some(ns)) = (callee_file, callee_name_start)
        && !file.is_empty()
        && let Some(&id) = def_map.get(&DefKey {
            file: file.to_string(),
            name_start: ns,
        })
        && let Some(f) = functions.get(id.index())
        && f.module == module
        && f.qualname == qualname
    {
        return Some(id);
    }
    qual_map
        .get(&(module.to_string(), qualname.to_string()))
        .copied()
}

pub(crate) fn resolve_project_ref(
    functions: &[FunctionFact],
    def_map: &DefMap,
    qual_map: &QualMap,
    r: &CachedProjectRef,
) -> Option<FunctionId> {
    resolve_project_callee(
        functions,
        def_map,
        qual_map,
        if r.file.is_empty() {
            None
        } else {
            Some(r.file.as_str())
        },
        Some(r.name_start),
        &r.module,
        &r.qualname,
    )
}
