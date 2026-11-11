//! Higher-order callable / offload-wrapper materialization before propagation.

use std::borrow::Cow;

use crate::graph::{CalleeRef, ProjectFacts};

fn needs_higher_order_materialization(facts: &ProjectFacts) -> bool {
    let has_offload_wrapper_fn = facts.functions.iter().any(|f| f.is_offload_wrapper);
    let has_offload_decorator_ref = facts.decorator_refs.values().flatten().any(|r| match r {
        crate::graph::DecoratorRef::External(q) => offload_decorator_qual(q).is_some(),
        crate::graph::DecoratorRef::Function(_) => false,
    });
    has_offload_decorator_ref
        || !facts.offload_wrapper_refs.is_empty()
        || has_offload_wrapper_fn
        || facts
            .edges
            .iter()
            .any(|edge| matches!(edge.callee, CalleeRef::ParamInvoke { .. }))
}

/// Link higher-order `ParamInvoke` edges using caller argument bindings, and
/// promote inferred offload wrappers onto call sites.
pub(super) fn materialize_higher_order(facts: &ProjectFacts) -> Cow<'_, ProjectFacts> {
    if !needs_higher_order_materialization(facts) {
        return Cow::Borrowed(facts);
    }
    let mut out = facts.clone();
    materialize_higher_order_in_place(&mut out);
    Cow::Owned(out)
}

/// In-place variant of [`materialize_higher_order`] so owned callers avoid
/// cloning [`ProjectFacts`] even when higher-order materialization is active.
pub(super) fn materialize_higher_order_owned(mut facts: ProjectFacts) -> ProjectFacts {
    if needs_higher_order_materialization(&facts) {
        materialize_higher_order_in_place(&mut facts);
    }
    facts
}

fn materialize_higher_order_in_place(out: &mut ProjectFacts) {
    use crate::graph::{CallEdge, OffloadKind};

    // `module.qualname` → id index for resolving `DecoratorRef::External`
    // quals to followed / project functions.
    let qual_index: std::collections::HashMap<String, crate::graph::FunctionId> =
        out.functions.iter().map(|f| (ref_key(f), f.id)).collect();
    let ref_is_offload_wrapper =
        |r: &crate::graph::DecoratorRef,
         funcs: &[crate::graph::FunctionFact],
         idx: &std::collections::HashMap<String, crate::graph::FunctionId>| {
            match r {
                crate::graph::DecoratorRef::Function(id) => funcs[id.index()].is_offload_wrapper,
                crate::graph::DecoratorRef::External(q) => idx
                    .get(q.as_str())
                    .is_some_and(|id| funcs[id.index()].is_offload_wrapper),
            }
        };

    // Deferred wrapper candidates: a nested, returned def forwards an
    // enclosing param to a callee that turned out to be an offload wrapper.
    // Iterate to a fixpoint so wrapper chains settle (bounded — marks only grow).
    loop {
        let mut to_mark = Vec::new();
        for (&fid, refs) in &out.offload_wrapper_refs {
            if out.functions[fid.index()].is_offload_wrapper {
                continue;
            }
            if refs
                .iter()
                .any(|r| ref_is_offload_wrapper(r, &out.functions, &qual_index))
            {
                to_mark.push(fid);
            }
        }
        if to_mark.is_empty() {
            break;
        }
        for fid in to_mark {
            out.functions[fid.index()].is_offload_wrapper = true;
        }
    }

    // Classify decorator refs once wrapper marks are final.
    let mut resolved_decorators = Vec::new();
    for (&fid, refs) in &out.decorator_refs {
        let kind = refs.iter().find_map(|r| match r {
            crate::graph::DecoratorRef::External(q) => offload_decorator_qual(q).or_else(|| {
                ref_is_offload_wrapper(r, &out.functions, &qual_index)
                    .then_some(OffloadKind::AsyncioToThread)
            }),
            crate::graph::DecoratorRef::Function(_) => {
                ref_is_offload_wrapper(r, &out.functions, &qual_index)
                    .then_some(OffloadKind::AsyncioToThread)
            }
        });
        if let Some(kind) = kind {
            resolved_decorators.push((fid, kind));
        }
    }
    for (fid, kind) in resolved_decorators {
        out.functions[fid.index()].offload_decorator = Some(kind);
    }

    // Index edges by caller once — avoid O(E²) scans.
    let mut by_caller: Vec<Vec<usize>> = vec![Vec::new(); out.functions.len()];
    for (i, edge) in out.edges.iter().enumerate() {
        let c = edge.caller.index();
        if c < by_caller.len() {
            by_caller[c].push(i);
        }
    }

    // Mark call edges to offload-decorated functions and inferred offload
    // wrappers as offload cuts.
    for edge in &mut out.edges {
        if let CalleeRef::Project(fid) = edge.callee {
            let f = &out.functions[fid.index()];
            if let Some(kind) = f.offload_decorator {
                // Calling a `@sync_to_async`-style decorated function runs its
                // body on a worker thread, never on this caller's loop.
                edge.is_offload_wrapper = true;
                edge.callee = CalleeRef::Offload(kind);
                // The offloaded callable is the decorated function itself;
                // call args are merely forwarded to it. Drop them so
                // `offload-coroutine` and twin advice read the decorated
                // function, not an argument.
                edge.callee_is_coroutine = Some(f.is_async);
                edge.positional_literals.clear();
                edge.keyword_true.clear();
                edge.positional_callables = vec![Some(fid)];
                edge.positional_externals.clear();
                edge.positional_constructed_receivers.clear();
                edge.keyword_callables.clear();
                edge.constructed_receiver = None;
                continue;
            }
            if !f.is_offload_wrapper {
                continue;
            }
            edge.is_offload_wrapper = true;
            edge.callee = CalleeRef::Offload(OffloadKind::AsyncioToThread);
            let arg_idx = usize::from(edge.is_bound_call);
            if arg_idx > 0 && edge.positional_callables.len() > arg_idx {
                edge.positional_callables.drain(0..arg_idx);
            }
            // The wrapper itself is often `async def`; coroutine-ness for
            // `offload-coroutine` must refer to the *callable argument*, not
            // the wrapper. Recompute from positional callables (index 0).
            edge.callee_is_coroutine = edge
                .positional_callables
                .first()
                .copied()
                .flatten()
                .map(|id| out.functions[id.index()].is_async);
        }
    }

    // Resolve ParamInvoke via caller's positional_callables / keyword_callables.
    let mut extras: Vec<CallEdge> = Vec::new();
    let n_edges = out.edges.len();
    for ei in 0..n_edges {
        if out.edges[ei].is_offload_wrapper {
            continue;
        }
        let CalleeRef::Project(callee_id) = out.edges[ei].callee else {
            continue;
        };
        let Some(param_edge_idxs) = by_caller.get(callee_id.index()) else {
            continue;
        };
        for &pei in param_edge_idxs {
            let CalleeRef::ParamInvoke { index } = out.edges[pei].callee else {
                continue;
            };
            let arg_idx = if out.edges[ei].is_bound_call {
                index.checked_sub(1)
            } else {
                Some(index)
            };
            let fid = arg_idx
                .and_then(|ai| {
                    out.edges[ei]
                        .positional_callables
                        .get(ai as usize)
                        .copied()
                        .flatten()
                })
                .or_else(|| {
                    let name = out.functions[callee_id.index()]
                        .param_names
                        .get(index as usize)?;
                    out.edges[ei]
                        .keyword_callables
                        .iter()
                        .find(|(k, _)| k == name)
                        .map(|(_, id)| *id)
                });
            let Some(fid) = fid else {
                continue;
            };
            let param_edge = &out.edges[pei];
            let already = by_caller
                .get(callee_id.index())
                .into_iter()
                .flatten()
                .any(|&j| {
                    matches!(out.edges[j].callee, CalleeRef::Project(id) if id == fid)
                        && out.edges[j].line == param_edge.line
                });
            if already {
                continue;
            }
            extras.push(CallEdge {
                caller: callee_id,
                callee: CalleeRef::Project(fid),
                file: param_edge.file.clone(),
                line: param_edge.line,
                text: param_edge.text.clone(),
                kind: param_edge.kind,
                conditions: param_edge.conditions.clone(),
                occurrence: param_edge.occurrence,
                is_offload_wrapper: false,
                positional_literals: vec![],
                keyword_true: vec![],
                positional_callables: vec![],
                positional_externals: vec![],
                positional_constructed_receivers: vec![],
                keyword_callables: vec![],
                param_guard: param_edge.param_guard,
                is_awaited: param_edge.is_awaited,
                callee_is_coroutine: param_edge.callee_is_coroutine,
                is_eager_offload_arg: param_edge.is_eager_offload_arg,
                is_bound_call: false,
                // Only valid when reverse-walk came from this exact call site.
                context_from_caller: Some(out.edges[ei].caller),
                context_from_edge: Some(ei),
                constructed_receiver: None,
            });
        }
    }
    out.edges.extend(extras);
}

/// Lookup key for `DecoratorRef::External` quals — `module.qualname` (or bare
/// `qualname` for script modules), matching ty's `module.Qual` shape.
fn ref_key(f: &crate::graph::FunctionFact) -> String {
    if f.module.is_empty() {
        f.qualname.clone()
    } else {
        format!("{}.{}", f.module, f.qualname)
    }
}

/// ty-resolved decorator qualnames that are offload boundaries. Bare and
/// called (`@sync_to_async(thread_sensitive=…)`) forms record the same qual.
fn offload_decorator_qual(q: &str) -> Option<crate::graph::OffloadKind> {
    use crate::graph::OffloadKind;
    match q {
        "asgiref.sync.sync_to_async" => Some(OffloadKind::AsgirefSyncToAsync),
        "channels.db.database_sync_to_async" => Some(OffloadKind::ChannelsDbSyncToAsync),
        _ => None,
    }
}
