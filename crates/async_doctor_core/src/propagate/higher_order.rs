//! Higher-order callable / offload-wrapper materialization before propagation.

use crate::graph::{CalleeRef, ProjectFacts};

/// Link higher-order `ParamInvoke` edges using caller argument bindings, and
/// promote inferred offload wrappers onto call sites.
pub(super) fn materialize_higher_order(facts: &ProjectFacts) -> ProjectFacts {
    use crate::graph::{CallEdge, OffloadKind};

    let mut out = facts.clone();

    // Index edges by caller once — avoid O(E²) scans.
    let mut by_caller: Vec<Vec<usize>> = vec![Vec::new(); out.functions.len()];
    for (i, edge) in out.edges.iter().enumerate() {
        let c = edge.caller.index();
        if c < by_caller.len() {
            by_caller[c].push(i);
        }
    }

    // Mark call edges to inferred offload wrappers as offload cuts.
    for edge in &mut out.edges {
        if let CalleeRef::Project(fid) = edge.callee
            && out.functions[fid.index()].is_offload_wrapper
        {
            edge.is_offload_wrapper = true;
            edge.callee = CalleeRef::Offload(OffloadKind::AsyncioToThread);
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
            let fid = out.edges[ei]
                .positional_callables
                .get(index as usize)
                .copied()
                .flatten()
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
                positional_callables: vec![],
                keyword_callables: vec![],
                param_guard: param_edge.param_guard,
                is_awaited: param_edge.is_awaited,
                callee_is_coroutine: param_edge.callee_is_coroutine,
                is_eager_offload_arg: param_edge.is_eager_offload_arg,
                // Only valid when reverse-walk came from this call site.
                context_from_caller: Some(out.edges[ei].caller),
            });
        }
    }
    out.edges.extend(extras);
    out
}
