//! Remap cached edges onto live def/qual maps and stabilize project facts.

use std::collections::HashMap;

use async_doctor_core::{
    CallEdge, CallKind, CalleeRef, ConditionTag, FunctionId, LiteralArg, OffloadKind, ParamGuard,
    ProjectFacts,
};

use super::ctx::DefKey;
use super::file_cache::CachedFileFacts;

/// Append edges from cached files using existing def/qual maps (no new functions).
pub(crate) fn append_cached_edges_only(
    facts: &mut ProjectFacts,
    cached: &[CachedFileFacts],
    def_map: &HashMap<DefKey, FunctionId>,
    qual_map: &HashMap<(String, String), FunctionId>,
) {
    let resolve_ref = |r: &super::file_cache::CachedProjectRef| -> Option<FunctionId> {
        if !r.file.is_empty()
            && let Some(&id) = def_map.get(&DefKey {
                file: r.file.clone(),
                name_start: r.name_start,
            })
        {
            return Some(id);
        }
        qual_map
            .get(&(r.module.clone(), r.qualname.clone()))
            .copied()
    };
    let resolve = |module: &str, qual: &str| -> Option<FunctionId> {
        qual_map
            .get(&(module.to_string(), qual.to_string()))
            .copied()
    };
    for cf in cached {
        for e in &cf.edges {
            let Some(&caller) = def_map.get(&DefKey {
                file: cf.rel.clone(),
                name_start: e.caller_name_start,
            }) else {
                continue;
            };
            let callee = match e.callee_kind.as_str() {
                "project" => {
                    let by_def = match (&e.callee_file, e.callee_name_start) {
                        (Some(file), Some(ns)) if !file.is_empty() => def_map
                            .get(&DefKey {
                                file: file.clone(),
                                name_start: ns,
                            })
                            .copied(),
                        _ => None,
                    };
                    let module = e.callee_module.as_deref().unwrap_or("");
                    match by_def.or_else(|| resolve(module, &e.callee_qualname)) {
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
                        Some("starlette.concurrency.run_in_threadpool") => {
                            OffloadKind::StarletteThreadpool
                        }
                        Some("concurrent.futures.ThreadPoolExecutor") => {
                            OffloadKind::ThreadPoolExecutor
                        }
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
            let parse_cond = |s: &str| match s {
                "unconditional" => Some(ConditionTag::Unconditional),
                "all-paths" => Some(ConditionTag::AllPaths),
                "loop" => Some(ConditionTag::Loop),
                "async-branch" => Some(ConditionTag::AsyncBranch),
                "cache_miss_dependent" => Some(ConditionTag::CacheMissDependent),
                "sync_entry" => Some(ConditionTag::SyncEntry),
                _ => None,
            };
            let parse_lit = |s: &str| match s {
                "true" => Some(LiteralArg::True),
                "false" => Some(LiteralArg::False),
                "none" => Some(LiteralArg::None),
                _ => None,
            };
            let parse_kind = |s: &str| match s {
                "WithEnter" => CallKind::WithEnter,
                "WithExit" => CallKind::WithExit,
                "ForIter" => CallKind::ForIter,
                "PropertyGet" => CallKind::PropertyGet,
                "GeneratorConsume" => CallKind::GeneratorConsume,
                _ => CallKind::Call,
            };
            facts.edges.push(CallEdge {
                caller,
                callee,
                file: e.file.clone(),
                line: e.line,
                text: e.text.clone(),
                kind: parse_kind(&e.kind),
                conditions: e.conditions.iter().filter_map(|s| parse_cond(s)).collect(),
                occurrence: e.occurrence,
                is_offload_wrapper: e.is_offload_wrapper,
                positional_literals: e
                    .positional_literals
                    .iter()
                    .map(|o| o.as_deref().and_then(parse_lit))
                    .collect(),
                positional_callables: e
                    .positional_callable_refs
                    .iter()
                    .map(|o| o.as_ref().and_then(resolve_ref))
                    .collect(),
                keyword_callables: e
                    .keyword_callable_refs
                    .iter()
                    .filter_map(|(k, r)| resolve_ref(r).map(|id| (k.clone(), id)))
                    .collect(),
                param_guard: match (e.param_guard_index, e.param_guard_inverted) {
                    (Some(index), Some(inverted)) => Some(ParamGuard { index, inverted }),
                    _ => None,
                },
                is_awaited: e.is_awaited,
                callee_is_coroutine: e.callee_is_coroutine,
                is_eager_offload_arg: e.is_eager_offload_arg,
                context_from_caller: e.context_from_caller.as_ref().and_then(resolve_ref),
            });
        }
    }
}

pub(crate) fn stabilize_facts(facts: &mut ProjectFacts) {
    facts
        .unresolved_imports
        .sort_by(|a, b| (&a.file, &a.module, a.line).cmp(&(&b.file, &b.module, b.line)));
    facts.unresolved_third_party_modules.sort();
    facts.unresolved_third_party_modules.dedup();

    let project_keys: Vec<String> = facts
        .functions
        .iter()
        .map(|f| format!("{}:{}", f.module, f.qualname))
        .collect();
    let callee_key = |e: &CallEdge| -> String {
        match &e.callee {
            CalleeRef::External { qualname } => format!("E:{qualname}"),
            CalleeRef::Unresolved { reason } => format!("U:{reason}"),
            CalleeRef::Offload(k) => format!("O:{}", k.as_str()),
            CalleeRef::ParamInvoke { index } => format!("I:{index}"),
            CalleeRef::Project(id) => format!(
                "P:{}",
                project_keys
                    .get(id.index())
                    .map(String::as_str)
                    .unwrap_or("")
            ),
        }
    };
    facts.edges.sort_by(|a, b| {
        (
            a.file.as_str(),
            a.line,
            a.occurrence,
            a.text.as_str(),
            callee_key(a),
            a.caller.0,
        )
            .cmp(&(
                b.file.as_str(),
                b.line,
                b.occurrence,
                b.text.as_str(),
                callee_key(b),
                b.caller.0,
            ))
    });
}
