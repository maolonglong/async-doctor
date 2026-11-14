//! Special finding rules: `nested-event-loop`, `offload-coroutine`,
//! `sync-with-async-cm`, `sync-for-async-iterable`, `unawaited-coroutine`,
//! and `eager-offload-call`.

use std::collections::{HashMap, HashSet};

use crate::graph::{
    CalleeRef, ConditionTag, FunctionFact, FunctionId, ProjectFacts, is_non_production_entry_path,
};
use crate::id::{FindingIdParts, fix_group_id};
use crate::report::{Entry, Finding, Fix};

use super::SinkHit;
use super::grade::{
    ClassInitIndex, ItemSpanMap, chain_step_for, collect_item_spans, reconcile_conditions,
    resolution_for_edge, shortest_paths_to_async_entries, unique_site_finding_id,
};

pub(super) const RULE_NESTED_EVENT_LOOP: &str = "nested-event-loop";
pub(super) const RULE_OFFLOAD_COROUTINE: &str = "offload-coroutine";
pub(super) const RULE_SYNC_WITH_ASYNC_CM: &str = "sync-with-async-cm";
pub(super) const RULE_SYNC_FOR_ASYNC_ITERABLE: &str = "sync-for-async-iterable";
pub(super) const RULE_EAGER_OFFLOAD_CALL: &str = "eager-offload-call";
pub(super) const RULE_UNAWAITED_COROUTINE: &str = "unawaited-coroutine";

/// Seed a special finding hit (`nested-event-loop` or first-party async anti-patterns).
pub(super) fn special_finding_hit(qualname: &str, caller: &FunctionFact) -> Option<SinkHit> {
    if !caller.is_installed_third_party() {
        if qualname == RULE_SYNC_WITH_ASYNC_CM {
            return Some(SinkHit {
                rule: RULE_SYNC_WITH_ASYNC_CM,
                sink_qualname: RULE_SYNC_WITH_ASYNC_CM,
                fix_hint: if caller.is_async {
                    "Use `async with ...:` instead of sync `with ...:` — \
                     this context manager only implements the async context protocol (`__aenter__` / `__aexit__`)."
                } else {
                    ""
                },
                candidate_only: false,
                candidate_reason: None,
                with_only: true,
                consume_only: false,
            });
        }
        if qualname == RULE_SYNC_FOR_ASYNC_ITERABLE {
            return Some(SinkHit {
                rule: RULE_SYNC_FOR_ASYNC_ITERABLE,
                sink_qualname: RULE_SYNC_FOR_ASYNC_ITERABLE,
                fix_hint: if caller.is_async {
                    "Use `async for ... in ...:` (or `[x async for x in ...]`) instead of \
                     sync iteration — this object only implements the async iterator protocol \
                     (`__aiter__` / `__anext__`)."
                } else {
                    ""
                },
                candidate_only: false,
                candidate_reason: None,
                with_only: false,
                consume_only: true,
            });
        }
        if qualname == RULE_UNAWAITED_COROUTINE {
            return Some(SinkHit {
                rule: RULE_UNAWAITED_COROUTINE,
                sink_qualname: RULE_UNAWAITED_COROUTINE,
                fix_hint: "",
                candidate_only: false,
                candidate_reason: None,
                with_only: false,
                consume_only: false,
            });
        }
    }
    if is_nested_event_loop_qual(qualname) {
        Some(SinkHit {
            rule: RULE_NESTED_EVENT_LOOP,
            sink_qualname: "nested-event-loop",
            fix_hint: "",
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

pub(super) struct SpecialFindingCtx<'a> {
    pub facts: &'a ProjectFacts,
    pub reverse: &'a [Vec<usize>],
    pub include_non_production: bool,
    pub class_inits: &'a ClassInitIndex,
    pub item_spans: &'a mut ItemSpanMap,
    pub emitted_sites: &'a mut HashMap<String, (FunctionId, String, u32)>,
    pub emitted: &'a mut HashSet<String>,
}

/// Emit `offload-coroutine` findings for coroutine targets, not async generators.
pub(super) fn collect_offload_coroutine_findings(ctx: &mut SpecialFindingCtx<'_>) -> Vec<Finding> {
    let facts = ctx.facts;
    let reverse = ctx.reverse;
    let include_non_production = ctx.include_non_production;
    let class_inits = ctx.class_inits;
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
            .map(|fid| facts.function(fid).is_coroutine())
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
            let Some(id) = unique_site_finding_id(
                FindingIdParts {
                    rule: RULE_OFFLOAD_COROUTINE,
                    entry_qualname: &entry.full_qualname(),
                    sink_qualname: RULE_OFFLOAD_COROUTINE,
                    chain_qualnames: &chain_refs,
                    occurrence: edge.occurrence,
                },
                entry_id,
                (&edge.file, edge.line),
                ctx.emitted_sites,
                ctx.emitted,
            ) else {
                continue;
            };
            ctx.item_spans.insert(
                id.clone(),
                collect_item_spans(facts, class_inits, &path, edge),
            );
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
                    hint: String::new(),
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

/// Emit `eager-offload-call` findings when an offload helper's callable slot is a
/// call expression returning a provably non-callable value (`to_thread(fn(...))`).
///
/// Deduplicates against `existing_findings` when the eagerly evaluated inner call
/// already produced a `blocking-*` finding for the same entry + offload site.
pub(super) fn collect_eager_offload_findings(
    ctx: &mut SpecialFindingCtx<'_>,
    existing_findings: &[Finding],
) -> Vec<Finding> {
    let facts = ctx.facts;
    let reverse = ctx.reverse;
    let include_non_production = ctx.include_non_production;
    let class_inits = ctx.class_inits;
    let mut out = Vec::new();
    for edge in &facts.edges {
        let CalleeRef::External { qualname } = &edge.callee else {
            continue;
        };
        let Some(rest) = qualname.strip_prefix("eager-offload-call:") else {
            continue;
        };
        if facts.function(edge.caller).is_installed_third_party() {
            continue;
        }
        let (offload_name, inner_line) = match rest.rsplit_once(':') {
            Some((name, line_str)) => (name, line_str.parse::<u32>().ok()),
            None => (rest, None),
        };

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

            // Avoid double-reporting when the eagerly evaluated argument already
            // produced a blocking-* finding through this offload site.
            let already_reported = existing_findings.iter().any(|f| {
                f.entry.file == entry.file
                    && f.entry.line == entry.line
                    && f.entry.name == entry.qualname
                    && f.chain.iter().any(|step| {
                        step.file == edge.file
                            && (step.line == edge.line || inner_line == Some(step.line))
                    })
            });
            if already_reported {
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
                    "eager call in `{offload_name}` callable slot — runs on event loop and passes non-callable result"
                ),
            ));
            condition_tags.extend(edge.conditions.iter().copied());
            chain_qualnames.push(format!("eager-offload-call:{offload_name}"));

            let conditions = reconcile_conditions(&condition_tags);
            let chain_refs: Vec<&str> = chain_qualnames.iter().map(String::as_str).collect();
            let Some(id) = unique_site_finding_id(
                FindingIdParts {
                    rule: RULE_EAGER_OFFLOAD_CALL,
                    entry_qualname: &entry.full_qualname(),
                    sink_qualname: RULE_EAGER_OFFLOAD_CALL,
                    chain_qualnames: &chain_refs,
                    occurrence: edge.occurrence,
                },
                entry_id,
                (&edge.file, edge.line),
                ctx.emitted_sites,
                ctx.emitted,
            ) else {
                continue;
            };
            ctx.item_spans.insert(
                id.clone(),
                collect_item_spans(facts, class_inits, &path, edge),
            );
            out.push(Finding {
                id,
                rule: RULE_EAGER_OFFLOAD_CALL.to_string(),
                entry: Entry {
                    file: entry.file.clone(),
                    line: entry.line,
                    name: entry.qualname.clone(),
                    label: entry.label,
                },
                chain: chain_steps,
                conditions,
                fix: Fix {
                    hint: String::new(),
                    fix_group_id: fix_group_id(RULE_EAGER_OFFLOAD_CALL, offload_name),
                },
                why: format!("async-doctor why {}:{}", edge.file, edge.line),
                docs_anchor: format!("#{RULE_EAGER_OFFLOAD_CALL}"),
                docs_url: format!(
                    "https://github.com/maolonglong/async-doctor/blob/main/docs/rules.md#{RULE_EAGER_OFFLOAD_CALL}"
                ),
            });
        }
    }
    out
}
