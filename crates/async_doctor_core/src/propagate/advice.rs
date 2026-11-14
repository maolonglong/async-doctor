//! Non-blocking advice: `offload-has-async-twin`.
//!
//! Fires only at offload cut sites reachable from async entries, when every
//! unconditionally reached sink has a high-confidence same-package async twin
//! that resolves (with its `needs`) in the scanned venv. Never affects exit code.
//! Symbol availability is evidence, not a migration recommendation or proof of
//! equivalent behavior or better performance.
//!
//! The offload site's own try/if/loop conditions are irrelevant — advice is about
//! the offloaded work's sinks, not whether the offload call is branch-guarded.

use std::collections::{HashMap, HashSet, VecDeque};

use crate::graph::{
    CalleeRef, FunctionId, OffloadKind, ProjectFacts, is_non_production_entry_path,
};
use crate::id::{FindingIdParts, fix_group_id};
use crate::report::{Advice, AdviceCategory, ChainStep, Entry, Fix};

use super::SinkTable;
use super::anti_pattern_advice::collect_anti_pattern_advice;
use super::grade::{
    ClassInitIndex, ItemSpanMap, chain_step_for, collect_item_spans, resolution_for_edge,
    shortest_paths_to_async_entries, unique_site_finding_id,
};
use super::sync_callers::{SyncCallerIndex, twin_lookup_key, twin_resolves};

pub(super) const RULE_OFFLOAD_HAS_ASYNC_TWIN: &str = "offload-has-async-twin";

/// Collect `offload-has-async-twin` advice items for the graded report.
pub(super) fn collect_offload_twin_advice(
    facts: &ProjectFacts,
    sinks: &dyn SinkTable,
    reverse: &[Vec<usize>],
    sync_callers: &SyncCallerIndex,
    include_non_production: bool,
    class_inits: &ClassInitIndex,
    item_spans: &mut ItemSpanMap,
) -> Vec<Advice> {
    let edges_by_caller = sync_callers.edges_by_caller();

    let mut advice = Vec::new();
    let mut emitted = HashSet::new();
    let mut emitted_sites: HashMap<String, (FunctionId, String, u32)> = HashMap::new();

    for (edge_idx, edge) in facts.edges.iter().enumerate() {
        let CalleeRef::Offload(kind) = edge.callee else {
            continue;
        };
        if !edge.is_offload_wrapper {
            continue;
        };
        // Eager `to_thread(f())` is a finding path, not a cut of `f`.
        if edge.is_eager_offload_arg {
            continue;
        }

        let paths = shortest_paths_to_async_entries(facts, reverse, edge.caller);
        if paths.is_empty() {
            continue;
        }

        let Some(target) = resolve_offload_target(facts, edge, kind) else {
            continue;
        };

        // Sync-caller exemption: target also invoked from sync code that is
        // not async-reachable — switching to an async twin would break it.
        if let OffloadTarget::Project(target_id) = &target
            && sync_callers.has_sync_only_caller(facts, *target_id, Some(edge_idx))
        {
            continue;
        }

        let Some(reached) = unconditional_sinks_from_target(facts, sinks, edges_by_caller, &target)
        else {
            continue;
        };
        if reached.is_empty() {
            continue;
        }

        // Every reached sink must have a high-confidence twin; all twins that
        // appear must resolve (symbol + needs) in the scanned venv.
        let mut twin_symbols: HashSet<&'static str> = HashSet::new();
        let mut ok = true;
        for sink_q in &reached {
            let Some(twin) = sinks.async_twin(sink_q) else {
                ok = false;
                break;
            };
            if !twin_resolves(facts, &twin) {
                ok = false;
                break;
            }
            twin_symbols.insert(twin.symbol);
        }
        if !ok || twin_symbols.is_empty() {
            continue;
        }

        // Keep all resolved symbols as evidence, sorted for cold/warm parity.
        let mut twin_list: Vec<&'static str> = twin_symbols.into_iter().collect();
        twin_list.sort_unstable();
        let twin_label = twin_list.join(" / ");
        let primary_twin = twin_list[0];

        for path in paths {
            let entry_id = path.entry;
            let entry = facts.function(entry_id);
            if entry.is_sync_route || !entry.is_async || entry.is_third_party {
                continue;
            }
            if !include_non_production && is_non_production_entry_path(&entry.file) {
                continue;
            }

            let mut chain_steps: Vec<ChainStep> = Vec::new();
            let mut chain_qualnames: Vec<String> = Vec::new();

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
            }

            let caller_fn = facts.function(edge.caller);
            chain_steps.push(chain_step_for(
                caller_fn,
                edge.file.clone(),
                edge.line,
                edge.text.clone(),
                format!(
                    "offload boundary (`{}`); same-package async APIs resolve in the scanned environment: `{twin_label}`",
                    kind.as_str()
                ),
            ));
            // Offload-site try/if/loop tags are intentionally ignored — only
            // unconditional sink reachability inside the target matters.
            chain_qualnames.push(format!("offload:{}", kind.as_str()));
            chain_qualnames.push(primary_twin.to_string());

            let chain_refs: Vec<&str> = chain_qualnames.iter().map(String::as_str).collect();
            let sink_key = format!("twin:{primary_twin}");
            let Some(id) = unique_site_finding_id(
                FindingIdParts {
                    rule: RULE_OFFLOAD_HAS_ASYNC_TWIN,
                    entry_qualname: &entry.full_qualname(),
                    sink_qualname: &sink_key,
                    chain_qualnames: &chain_refs,
                    occurrence: edge.occurrence,
                },
                entry_id,
                (&edge.file, edge.line),
                &mut emitted_sites,
                &mut emitted,
            ) else {
                continue;
            };
            item_spans.insert(
                id.clone(),
                collect_item_spans(facts, class_inits, &path, edge),
            );

            advice.push(Advice {
                id,
                rule: RULE_OFFLOAD_HAS_ASYNC_TWIN.to_string(),
                category: AdviceCategory::Performance,
                entry: Entry {
                    file: entry.file.clone(),
                    line: entry.line,
                    name: entry.qualname.clone(),
                    label: entry.label,
                },
                chain: chain_steps,
                fix: Fix {
                    hint: String::new(),
                    // Group by offload site + twin so text can cluster them.
                    fix_group_id: fix_group_id(
                        RULE_OFFLOAD_HAS_ASYNC_TWIN,
                        &format!("{}:{}:{primary_twin}", edge.file, edge.line),
                    ),
                },
                why: format!("async-doctor why {}:{}", edge.file, edge.line),
                docs_anchor: format!("#{RULE_OFFLOAD_HAS_ASYNC_TWIN}"),
                docs_url: format!(
                    "https://github.com/maolonglong/async-doctor/blob/main/docs/rules.md#{RULE_OFFLOAD_HAS_ASYNC_TWIN}"
                ),
            });
        }
    }

    collect_anti_pattern_advice(
        facts,
        reverse,
        include_non_production,
        class_inits,
        item_spans,
        &mut advice,
        &mut emitted_sites,
        &mut emitted,
    );

    // Sort by offload site file/line (last chain hop), not fixGroupId hash.
    advice.sort_by(|a, b| {
        let (af, al) = match a.chain.last() {
            Some(s) => (s.file.as_str(), s.line),
            None => (a.entry.file.as_str(), a.entry.line),
        };
        let (bf, bl) = match b.chain.last() {
            Some(s) => (s.file.as_str(), s.line),
            None => (b.entry.file.as_str(), b.entry.line),
        };
        (af, al, a.id.as_str()).cmp(&(bf, bl, b.id.as_str()))
    });
    advice
}

#[derive(Debug, Clone)]
enum OffloadTarget {
    Project(FunctionId),
    External {
        qualname: String,
        /// Root constructed client (`openai.AzureOpenAI`) when the offloaded
        /// callable is a bound method on a typed client — overrides shared
        /// `openai.resources.*` → AsyncOpenAI twin selection.
        constructed_receiver: Option<String>,
    },
}

fn resolve_offload_target(
    facts: &ProjectFacts,
    edge: &crate::graph::CallEdge,
    kind: OffloadKind,
) -> Option<OffloadTarget> {
    let idx = kind.callable_arg_index();
    if let Some(Some(fid)) = edge.positional_callables.get(idx).copied() {
        // Offloading an async def is a finding (`offload-coroutine`), not advice.
        if facts.function(fid).is_async {
            return None;
        }
        return Some(OffloadTarget::Project(fid));
    }
    if let Some(Some(q)) = edge.positional_externals.get(idx) {
        let constructed_receiver = edge
            .positional_constructed_receivers
            .get(idx)
            .and_then(|r| r.clone());
        return Some(OffloadTarget::External {
            qualname: q.clone(),
            constructed_receiver,
        });
    }
    None
}

/// Forward walk from the offload target collecting unconditional External sinks.
///
/// Returns `None` when a non-twin-eligible / excluded sink is reached, or when
/// the walk hits unresolved/dynamic dispatch that would make "all sinks have
/// twins" unprovable.
fn unconditional_sinks_from_target(
    facts: &ProjectFacts,
    sinks: &dyn SinkTable,
    edges_by_caller: &HashMap<FunctionId, Vec<usize>>,
    target: &OffloadTarget,
) -> Option<Vec<String>> {
    let mut sink_quals: Vec<String> = Vec::new();
    let mut seen_sinks = HashSet::new();

    match target {
        OffloadTarget::External {
            qualname: q,
            constructed_receiver,
        } => {
            // Direct offload of a typed external callable — that callable is the sink.
            if sinks.lookup(q).is_none() {
                return Some(vec![]);
            }
            let twin_key = twin_lookup_key(sinks, q, constructed_receiver.as_deref())?;
            sink_quals.push(twin_key);
            return Some(sink_quals);
        }
        OffloadTarget::Project(start) => {
            let mut visited = HashSet::new();
            let mut queue = VecDeque::new();
            queue.push_back(*start);
            visited.insert(*start);

            while let Some(fid) = queue.pop_front() {
                let Some(edge_idxs) = edges_by_caller.get(&fid) else {
                    continue;
                };
                for &eidx in edge_idxs {
                    let edge = &facts.edges[eidx];
                    // Only unconditional edges count (relative to the target /
                    // lambda body — enclosing offload-site tags are not here).
                    if edge.conditions.iter().any(|c| c.is_branch_like()) {
                        continue;
                    }
                    if !edge.param_guards.is_empty() {
                        continue;
                    }
                    match &edge.callee {
                        CalleeRef::Project(callee) => {
                            if visited.insert(*callee) {
                                queue.push_back(*callee);
                            }
                        }
                        CalleeRef::External { qualname } => {
                            if edge.callee_is_coroutine == Some(true) {
                                continue;
                            }
                            let Some(hit) = sinks.lookup(qualname) else {
                                continue;
                            };
                            if hit.with_only
                                && !matches!(edge.kind, crate::graph::CallKind::WithEnter)
                            {
                                continue;
                            }
                            if hit.consume_only
                                && !matches!(
                                    edge.kind,
                                    crate::graph::CallKind::GeneratorConsume
                                        | crate::graph::CallKind::ForIter
                                )
                            {
                                continue;
                            }
                            let twin_key = twin_lookup_key(
                                sinks,
                                qualname,
                                edge.constructed_receiver.as_deref(),
                            )?;
                            if seen_sinks.insert(twin_key.clone()) {
                                sink_quals.push(twin_key);
                            }
                        }
                        CalleeRef::Unresolved { .. } => {
                            // Cannot prove "all sinks have twins".
                            return None;
                        }
                        CalleeRef::Offload(_) | CalleeRef::ParamInvoke { .. } => {}
                    }
                }
            }
        }
    }

    Some(sink_quals)
}
