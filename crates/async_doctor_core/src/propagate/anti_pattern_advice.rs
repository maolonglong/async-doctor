//! First-party async anti-pattern advice rules:
//! - `dangling-task` (correctness)
//! - `ephemeral-async-client` (performance)
//! - `shared-async-session` (correctness)
//! - `offload-stateful-session` / `useless-local-async-lock` / `swallowed-cancelled-error` (see `llm_rules`)

mod llm_rules;

use std::collections::{HashMap, HashSet, VecDeque};

use crate::graph::{
    CalleeRef, ConditionTag, FunctionId, ProjectFacts, is_non_production_entry_path,
};
use crate::id::{FindingIdParts, fix_group_id};
use crate::report::{Advice, AdviceCategory, ChainStep, Entry, EntryLabel, Fix};

use super::grade::{
    ClassInitIndex, ItemSpanMap, PathToEntry, chain_step_for, collect_item_spans,
    resolution_for_edge, shortest_paths_to_async_entries, unique_site_finding_id,
};

pub(super) const RULE_DANGLING_TASK: &str = "dangling-task";
pub(super) const RULE_EPHEMERAL_ASYNC_CLIENT: &str = "ephemeral-async-client";
pub(super) const RULE_SHARED_ASYNC_SESSION: &str = "shared-async-session";

#[allow(clippy::too_many_arguments)]
pub(super) fn collect_anti_pattern_advice(
    facts: &ProjectFacts,
    reverse: &[Vec<usize>],
    include_non_production: bool,
    class_inits: &ClassInitIndex,
    item_spans: &mut ItemSpanMap,
    advice: &mut Vec<Advice>,
    emitted_sites: &mut HashMap<String, (FunctionId, String, u32)>,
    emitted: &mut HashSet<String>,
) {
    collect_dangling_task_advice(
        facts,
        reverse,
        include_non_production,
        class_inits,
        item_spans,
        advice,
        emitted_sites,
        emitted,
    );
    collect_ephemeral_async_client_advice(
        facts,
        reverse,
        include_non_production,
        class_inits,
        item_spans,
        advice,
        emitted_sites,
        emitted,
    );
    collect_shared_async_session_advice(
        facts,
        reverse,
        include_non_production,
        class_inits,
        item_spans,
        advice,
        emitted_sites,
        emitted,
    );
    llm_rules::collect_llm_advice(
        facts,
        reverse,
        include_non_production,
        class_inits,
        item_spans,
        advice,
        emitted_sites,
        emitted,
    );
}

#[allow(clippy::too_many_arguments)]
fn collect_dangling_task_advice(
    facts: &ProjectFacts,
    reverse: &[Vec<usize>],
    include_non_production: bool,
    class_inits: &ClassInitIndex,
    item_spans: &mut ItemSpanMap,
    advice: &mut Vec<Advice>,
    emitted_sites: &mut HashMap<String, (FunctionId, String, u32)>,
    emitted: &mut HashSet<String>,
) {
    for edge in &facts.edges {
        let CalleeRef::External { qualname } = &edge.callee else {
            continue;
        };
        if qualname != RULE_DANGLING_TASK {
            continue;
        }
        if facts.function(edge.caller).is_installed_third_party() {
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

            let (chain_steps, chain_qualnames) = build_advice_chain(
                facts,
                &path,
                edge,
                "fire-and-forget task (`Task` reference discarded)".to_string(),
                RULE_DANGLING_TASK.to_string(),
            );
            let chain_refs: Vec<&str> = chain_qualnames.iter().map(String::as_str).collect();
            let Some(id) = unique_site_finding_id(
                FindingIdParts {
                    rule: RULE_DANGLING_TASK,
                    entry_qualname: &entry.full_qualname(),
                    sink_qualname: RULE_DANGLING_TASK,
                    chain_qualnames: &chain_refs,
                    occurrence: edge.occurrence,
                },
                entry_id,
                (&edge.file, edge.line),
                emitted_sites,
                emitted,
            ) else {
                continue;
            };
            item_spans.insert(
                id.clone(),
                collect_item_spans(facts, class_inits, &path, edge),
            );
            advice.push(Advice {
                id,
                rule: RULE_DANGLING_TASK.to_string(),
                category: AdviceCategory::Correctness,
                entry: Entry {
                    file: entry.file.clone(),
                    line: entry.line,
                    name: entry.qualname.clone(),
                    label: entry.label,
                },
                chain: chain_steps,
                fix: Fix {
                    hint: String::new(),
                    fix_group_id: fix_group_id(RULE_DANGLING_TASK, RULE_DANGLING_TASK),
                },
                why: format!("async-doctor why {}:{}", edge.file, edge.line),
                docs_anchor: "#dangling-task".into(),
                docs_url:
                    "https://github.com/maolonglong/async-doctor/blob/main/docs/rules.md#dangling-task"
                        .into(),
            });
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn collect_ephemeral_async_client_advice(
    facts: &ProjectFacts,
    reverse: &[Vec<usize>],
    include_non_production: bool,
    class_inits: &ClassInitIndex,
    item_spans: &mut ItemSpanMap,
    advice: &mut Vec<Advice>,
    emitted_sites: &mut HashMap<String, (FunctionId, String, u32)>,
    emitted: &mut HashSet<String>,
) {
    for edge in &facts.edges {
        let CalleeRef::External { qualname } = &edge.callee else {
            continue;
        };
        let Some(client_name) = qualname.strip_prefix("ephemeral-async-client:") else {
            continue;
        };
        if facts.function(edge.caller).is_installed_third_party() {
            continue;
        }
        let seed_has_loop = edge.conditions.contains(&ConditionTag::Loop);
        let paths =
            shortest_paths_to_route_or_loop_entries(facts, reverse, edge.caller, seed_has_loop);
        for path in paths {
            let entry_id = path.entry;
            let entry = facts.function(entry_id);
            if entry.is_sync_route
                || !entry.is_async
                || entry.is_third_party
                || entry.label == EntryLabel::Lifecycle
            {
                continue;
            }
            if !include_non_production && is_non_production_entry_path(&entry.file) {
                continue;
            }

            let sink_key = format!("{RULE_EPHEMERAL_ASYNC_CLIENT}:{client_name}");
            let (chain_steps, chain_qualnames) = build_advice_chain(
                facts,
                &path,
                edge,
                format!("short-lived `{client_name}` created and closed per call/iteration"),
                sink_key.clone(),
            );
            let chain_refs: Vec<&str> = chain_qualnames.iter().map(String::as_str).collect();
            let Some(id) = unique_site_finding_id(
                FindingIdParts {
                    rule: RULE_EPHEMERAL_ASYNC_CLIENT,
                    entry_qualname: &entry.full_qualname(),
                    sink_qualname: &sink_key,
                    chain_qualnames: &chain_refs,
                    occurrence: edge.occurrence,
                },
                entry_id,
                (&edge.file, edge.line),
                emitted_sites,
                emitted,
            ) else {
                continue;
            };
            item_spans.insert(
                id.clone(),
                collect_item_spans(facts, class_inits, &path, edge),
            );
            advice.push(Advice {
                id,
                rule: RULE_EPHEMERAL_ASYNC_CLIENT.to_string(),
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
                    fix_group_id: fix_group_id(RULE_EPHEMERAL_ASYNC_CLIENT, client_name),
                },
                why: format!("async-doctor why {}:{}", edge.file, edge.line),
                docs_anchor: format!("#{RULE_EPHEMERAL_ASYNC_CLIENT}"),
                docs_url: format!(
                    "https://github.com/maolonglong/async-doctor/blob/main/docs/rules.md#{RULE_EPHEMERAL_ASYNC_CLIENT}"
                ),
            });
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn collect_shared_async_session_advice(
    facts: &ProjectFacts,
    reverse: &[Vec<usize>],
    include_non_production: bool,
    class_inits: &ClassInitIndex,
    item_spans: &mut ItemSpanMap,
    advice: &mut Vec<Advice>,
    emitted_sites: &mut HashMap<String, (FunctionId, String, u32)>,
    emitted: &mut HashSet<String>,
) {
    for edge in &facts.edges {
        let CalleeRef::External { qualname } = &edge.callee else {
            continue;
        };
        let Some(handle_name) = qualname.strip_prefix("shared-async-session:") else {
            continue;
        };
        if facts.function(edge.caller).is_installed_third_party() {
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

            let sink_key = format!("{RULE_SHARED_ASYNC_SESSION}:{handle_name}");
            let (chain_steps, chain_qualnames) = build_advice_chain(
                facts,
                &path,
                edge,
                format!(
                    "stateful session/connection `{handle_name}` shared across concurrent tasks"
                ),
                sink_key.clone(),
            );
            let chain_refs: Vec<&str> = chain_qualnames.iter().map(String::as_str).collect();
            let Some(id) = unique_site_finding_id(
                FindingIdParts {
                    rule: RULE_SHARED_ASYNC_SESSION,
                    entry_qualname: &entry.full_qualname(),
                    sink_qualname: &sink_key,
                    chain_qualnames: &chain_refs,
                    occurrence: edge.occurrence,
                },
                entry_id,
                (&edge.file, edge.line),
                emitted_sites,
                emitted,
            ) else {
                continue;
            };
            item_spans.insert(
                id.clone(),
                collect_item_spans(facts, class_inits, &path, edge),
            );
            advice.push(Advice {
                id,
                rule: RULE_SHARED_ASYNC_SESSION.to_string(),
                category: AdviceCategory::Correctness,
                entry: Entry {
                    file: entry.file.clone(),
                    line: entry.line,
                    name: entry.qualname.clone(),
                    label: entry.label,
                },
                chain: chain_steps,
                fix: Fix {
                    hint: String::new(),
                    fix_group_id: fix_group_id(RULE_SHARED_ASYNC_SESSION, RULE_SHARED_ASYNC_SESSION),
                },
                why: format!("async-doctor why {}:{}", edge.file, edge.line),
                docs_anchor: format!("#{RULE_SHARED_ASYNC_SESSION}"),
                docs_url: format!(
                    "https://github.com/maolonglong/async-doctor/blob/main/docs/rules.md#{RULE_SHARED_ASYNC_SESSION}"
                ),
            });
        }
    }
}

pub(super) fn build_advice_chain(
    facts: &ProjectFacts,
    path: &PathToEntry,
    edge: &crate::graph::CallEdge,
    terminal_resolution: String,
    terminal_qualname: String,
) -> (Vec<ChainStep>, Vec<String>) {
    let mut chain_steps = Vec::new();
    let mut chain_qualnames = Vec::new();
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
        terminal_resolution,
    ));
    chain_qualnames.push(terminal_qualname);
    (chain_steps, chain_qualnames)
}

/// Find reverse paths for `ephemeral-async-client`:
/// - Never crosses `EntryLabel::Lifecycle` or installed third-party async.
/// - Emits when reaching a `Route` async entry, or any non-`Lifecycle` async
///   entry once `ConditionTag::Loop` has been seen along the path.
/// - Walks *through* intermediate `EntryLabel::Other` / `Task` async helpers
///   when no loop has been seen yet so `route -> helper -> AsyncClient()` still
///   attributes to `route` while `main -> helper -> AsyncClient()` stays silent.
fn shortest_paths_to_route_or_loop_entries(
    facts: &ProjectFacts,
    reverse: &[Vec<usize>],
    seed_caller: FunctionId,
    seed_has_loop: bool,
) -> Vec<PathToEntry> {
    let mut entries = Vec::new();
    let seed_fn = facts.function(seed_caller);
    if seed_fn.label == EntryLabel::Lifecycle {
        return entries;
    }
    if seed_fn.is_async
        && !seed_fn.is_sync_route
        && !seed_fn.is_third_party
        && (seed_fn.label == EntryLabel::Route || seed_has_loop)
    {
        entries.push(PathToEntry {
            entry: seed_caller,
            edges: vec![],
        });
        if !is_non_production_entry_path(&seed_fn.file) {
            return entries;
        }
    }

    type State = (FunctionId, bool);
    let mut via: HashMap<State, (State, usize)> = HashMap::new();
    let mut visited: HashSet<State> = HashSet::new();
    let mut queue: VecDeque<State> = VecDeque::new();
    let seed_state: State = (seed_caller, seed_has_loop);
    queue.push_back(seed_state);
    visited.insert(seed_state);

    while let Some((node, has_loop)) = queue.pop_front() {
        let Some(preds) = reverse.get(node.index()) else {
            continue;
        };
        for &edge_idx in preds {
            let edge = &facts.edges[edge_idx];
            let pred = edge.caller;
            let pred_fn = facts.function(pred);
            if pred_fn.label == EntryLabel::Lifecycle
                || (pred_fn.is_installed_third_party() && pred_fn.is_async)
            {
                continue;
            }
            let next_loop = has_loop || edge.conditions.contains(&ConditionTag::Loop);
            let pred_state: State = (pred, next_loop);
            if !visited.insert(pred_state) {
                continue;
            }
            via.insert(pred_state, ((node, has_loop), edge_idx));
            if pred_fn.is_async
                && !pred_fn.is_sync_route
                && !pred_fn.is_third_party
                && (pred_fn.label == EntryLabel::Route || next_loop)
            {
                let mut edges = Vec::new();
                let mut cur = pred_state;
                while cur != seed_state {
                    let Some(&(child, eidx)) = via.get(&cur) else {
                        break;
                    };
                    edges.push(eidx);
                    cur = child;
                }
                entries.push(PathToEntry { entry: pred, edges });
                if is_non_production_entry_path(&pred_fn.file) {
                    queue.push_back(pred_state);
                }
            } else {
                queue.push_back(pred_state);
            }
        }
    }

    entries
}
