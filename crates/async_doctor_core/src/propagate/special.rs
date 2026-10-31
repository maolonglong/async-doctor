//! Special finding rules: `nested-event-loop` and `offload-coroutine`.

use std::collections::{BTreeMap, HashSet};

use crate::graph::{
    CalleeRef, ConditionTag, FunctionId, ProjectFacts, is_non_production_entry_path,
};
use crate::id::{FindingIdParts, finding_id, fix_group_id};
use crate::report::{Entry, Finding, Fix};

use super::grade::{
    chain_step_for, reconcile_conditions, resolution_for_edge, shortest_paths_to_async_entries,
};
use super::SinkHit;

pub(super) const RULE_NESTED_EVENT_LOOP: &str = "nested-event-loop";
pub(super) const RULE_OFFLOAD_COROUTINE: &str = "offload-coroutine";

/// Seed a nested-event-loop hit when the external qualname is a nested-loop API.
pub(super) fn nested_event_loop_hit(qualname: &str) -> Option<SinkHit> {
    if is_nested_event_loop_qual(qualname) {
        Some(SinkHit {
            rule: RULE_NESTED_EVENT_LOOP,
            sink_qualname: "nested-event-loop",
            fix_hint: "Do not call `asyncio.run` / `loop.run_until_complete` / `anyio.run` / \
                       `trio.run` from async code — you are already inside a running loop. \
                       Await the coroutine directly, or offload truly sync work with \
                       `asyncio.to_thread`.",
            candidate_only: false,
            candidate_reason: None,
            with_only: false,
            consume_only: false,
        })
    } else {
        None
    }
}

pub(super) fn is_nested_event_loop_qual(qualname: &str) -> bool {
    matches!(
        qualname,
        "asyncio.run"
            | "asyncio.runners.run"
            | "asyncio.events.AbstractEventLoop.run_until_complete"
            | "asyncio.events.AbstractEventLoop.run_forever"
            | "asyncio.base_events.BaseEventLoop.run_until_complete"
            | "asyncio.base_events.BaseEventLoop.run_forever"
            | "asyncio.Runner.run"
            | "asyncio.runners.Runner.run"
            | "anyio.run"
            | "trio.run"
    ) || qualname.ends_with(".run_until_complete")
        || qualname.ends_with(".run_forever")
        || (qualname.starts_with("asyncio.") && qualname.ends_with(".Runner.run"))
}

/// Emit `offload-coroutine` findings for offload sites whose target is `async def`.
pub(super) fn collect_offload_coroutine_findings(
    facts: &ProjectFacts,
    reverse: &BTreeMap<FunctionId, Vec<usize>>,
    include_non_production: bool,
    emitted: &mut HashSet<String>,
) -> Vec<Finding> {
    let mut out = Vec::new();
    for edge in &facts.edges {
        let CalleeRef::Offload(kind) = edge.callee else {
            continue;
        };
        if !edge.is_offload_wrapper {
            continue;
        }

        // Prefer the offloaded callable's async-ness (positional / stored
        // coroutine fact). Do not treat an async *wrapper* as the target.
        let idx = kind.callable_arg_index();
        let is_coro = edge
            .positional_callables
            .get(idx)
            .copied()
            .flatten()
            .map(|fid| facts.function(fid).is_async)
            .or(edge.callee_is_coroutine)
            .unwrap_or(false);
        if !is_coro {
            continue;
        }

        let paths = shortest_paths_to_async_entries(facts, reverse, edge.caller);
        for path in paths {
            let entry_id = path.entry;
            let entry = facts.function(entry_id);
            if entry.is_sync_route || !entry.is_async || entry.is_third_party {
                continue;
            }
            if !include_non_production && is_non_production_entry_path(&entry.file) {
                continue;
            }

            let mut chain_steps = Vec::new();
            let mut chain_qualnames = Vec::new();
            let mut condition_tags: Vec<ConditionTag> = Vec::new();
            for &eidx in &path.edges {
                let e = &facts.edges[eidx];
                let caller_fn = facts.function(e.caller);
                chain_steps.push(chain_step_for(
                    caller_fn,
                    e.file.clone(),
                    e.line,
                    e.text.clone(),
                    resolution_for_edge(facts, e),
                ));
                if let CalleeRef::Project(fid) = e.callee {
                    chain_qualnames.push(facts.function(fid).full_qualname());
                }
                condition_tags.extend(e.conditions.iter().copied());
            }
            let caller_fn = facts.function(edge.caller);
            chain_steps.push(chain_step_for(
                caller_fn,
                edge.file.clone(),
                edge.line,
                edge.text.clone(),
                format!(
                    "offload of async def (`{}`) — coroutine never runs",
                    kind.as_str()
                ),
            ));
            condition_tags.extend(edge.conditions.iter().copied());
            chain_qualnames.push(format!("offload-coroutine:{}", kind.as_str()));

            let conditions = reconcile_conditions(&condition_tags);
            let chain_refs: Vec<&str> = chain_qualnames.iter().map(String::as_str).collect();
            let id = finding_id(FindingIdParts {
                rule: RULE_OFFLOAD_COROUTINE,
                entry_qualname: &entry.full_qualname(),
                sink_qualname: RULE_OFFLOAD_COROUTINE,
                chain_qualnames: &chain_refs,
                occurrence: edge.occurrence,
            });
            if !emitted.insert(id.clone()) {
                continue;
            }
            out.push(Finding {
                id,
                rule: RULE_OFFLOAD_COROUTINE.to_string(),
                entry: Entry {
                    file: entry.file.clone(),
                    line: entry.line,
                    name: entry.qualname.clone(),
                    label: entry.label,
                },
                chain: chain_steps,
                conditions,
                fix: Fix {
                    hint: format!(
                        "`{}` expects a sync callable. Passing an `async def` builds a \
                         coroutine that is never awaited — the work does not run. Await \
                         the coroutine directly, or pass a sync function.",
                        kind.as_str()
                    ),
                    fix_group_id: fix_group_id(RULE_OFFLOAD_COROUTINE, kind.as_str()),
                },
                why: format!("async-doctor why {}:{}", edge.file, edge.line),
                docs_anchor: format!("#{RULE_OFFLOAD_COROUTINE}"),
                docs_url: format!(
                    "https://github.com/maolonglong/async-doctor/blob/main/docs/rules.md#{RULE_OFFLOAD_COROUTINE}"
                ),
            });
        }
    }
    out
}
