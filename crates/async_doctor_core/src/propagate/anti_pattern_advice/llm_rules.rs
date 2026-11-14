//! Propagation for LLM async correctness advice rules:
//! - `offload-stateful-session`
//! - `useless-local-async-lock`
//! - `swallowed-cancelled-error`

use std::collections::{HashMap, HashSet};

use crate::graph::{CalleeRef, FunctionId, ProjectFacts, is_non_production_entry_path};
use crate::id::{FindingIdParts, fix_group_id};
use crate::report::{Advice, AdviceCategory, Entry, EntryLabel, Fix};

use super::super::grade::{
    ClassInitIndex, ItemSpanMap, collect_item_spans, shortest_paths_to_async_entries,
    unique_site_finding_id,
};
use super::build_advice_chain;

pub(super) const RULE_OFFLOAD_STATEFUL_SESSION: &str = "offload-stateful-session";
pub(super) const RULE_USELESS_LOCAL_ASYNC_LOCK: &str = "useless-local-async-lock";
pub(super) const RULE_SWALLOWED_CANCELLED_ERROR: &str = "swallowed-cancelled-error";

#[allow(clippy::too_many_arguments)]
pub(super) fn collect_llm_advice(
    facts: &ProjectFacts,
    reverse: &[Vec<usize>],
    include_non_production: bool,
    class_inits: &ClassInitIndex,
    item_spans: &mut ItemSpanMap,
    advice: &mut Vec<Advice>,
    emitted_sites: &mut HashMap<String, (FunctionId, String, u32)>,
    emitted: &mut HashSet<String>,
) {
    collect_offload_stateful_session_advice(
        facts,
        reverse,
        include_non_production,
        class_inits,
        item_spans,
        advice,
        emitted_sites,
        emitted,
    );
    collect_useless_local_async_lock_advice(
        facts,
        reverse,
        include_non_production,
        class_inits,
        item_spans,
        advice,
        emitted_sites,
        emitted,
    );
    collect_swallowed_cancelled_error_advice(
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
fn collect_offload_stateful_session_advice(
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
        let Some(rest) = qualname.strip_prefix("offload-stateful-session:") else {
            continue;
        };
        let Some((offload_name, handle_name)) = rest.split_once(':') else {
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

            let sink_key = format!("{RULE_OFFLOAD_STATEFUL_SESSION}:{handle_name}");
            let (chain_steps, chain_qualnames) = build_advice_chain(
                facts,
                &path,
                edge,
                format!(
                    "bound `{handle_name}` method dispatched across thread boundary via `{offload_name}`"
                ),
                sink_key.clone(),
            );
            let chain_refs: Vec<&str> = chain_qualnames.iter().map(String::as_str).collect();
            let Some(id) = unique_site_finding_id(
                FindingIdParts {
                    rule: RULE_OFFLOAD_STATEFUL_SESSION,
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
                rule: RULE_OFFLOAD_STATEFUL_SESSION.to_string(),
                category: AdviceCategory::Correctness,
                entry: Entry {
                    file: entry.file.clone(),
                    line: entry.line,
                    name: entry.qualname.clone(),
                    label: entry.label,
                },
                chain: chain_steps,
                fix: Fix {
                    hint: format!(
                        "`{handle_name}` is a stateful sync handle that is not safe to dispatch \
                         across worker threads via `{offload_name}` (sync `Session` state is not \
                         thread-safe, and `sqlite3` defaults to `check_same_thread=True`). Offload \
                         a self-contained sync function that opens, commits, and closes its own \
                         session/connection inside the worker thread, or switch to an async driver."
                    ),
                    fix_group_id: fix_group_id(RULE_OFFLOAD_STATEFUL_SESSION, handle_name),
                },
                why: format!("async-doctor why {}:{}", edge.file, edge.line),
                docs_anchor: format!("#{RULE_OFFLOAD_STATEFUL_SESSION}"),
                docs_url: format!(
                    "https://github.com/maolonglong/async-doctor/blob/main/docs/rules.md#{RULE_OFFLOAD_STATEFUL_SESSION}"
                ),
            });
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn collect_useless_local_async_lock_advice(
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
        let Some(lock_name) = qualname.strip_prefix("useless-local-async-lock:") else {
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

            let sink_key = format!("{RULE_USELESS_LOCAL_ASYNC_LOCK}:{lock_name}");
            let (chain_steps, chain_qualnames) = build_advice_chain(
                facts,
                &path,
                edge,
                format!("unshared `{lock_name}` constructed per call (no cross-call contention)"),
                sink_key.clone(),
            );
            let chain_refs: Vec<&str> = chain_qualnames.iter().map(String::as_str).collect();
            let Some(id) = unique_site_finding_id(
                FindingIdParts {
                    rule: RULE_USELESS_LOCAL_ASYNC_LOCK,
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
                rule: RULE_USELESS_LOCAL_ASYNC_LOCK.to_string(),
                category: AdviceCategory::Correctness,
                entry: Entry {
                    file: entry.file.clone(),
                    line: entry.line,
                    name: entry.qualname.clone(),
                    label: entry.label,
                },
                chain: chain_steps,
                fix: Fix {
                    hint: format!(
                        "`{lock_name}` is freshly constructed on every call and never shared with \
                         another coroutine or closure, so `async with` provides zero mutual \
                         exclusion or concurrency limiting. Store the primitive once on shared \
                         state (`self._lock` in `__init__`, lifespan state, or module scope)."
                    ),
                    fix_group_id: fix_group_id(RULE_USELESS_LOCAL_ASYNC_LOCK, lock_name),
                },
                why: format!("async-doctor why {}:{}", edge.file, edge.line),
                docs_anchor: format!("#{RULE_USELESS_LOCAL_ASYNC_LOCK}"),
                docs_url: format!(
                    "https://github.com/maolonglong/async-doctor/blob/main/docs/rules.md#{RULE_USELESS_LOCAL_ASYNC_LOCK}"
                ),
            });
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn collect_swallowed_cancelled_error_advice(
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
        let Some(catch_kind) = qualname.strip_prefix("swallowed-cancelled-error:") else {
            continue;
        };
        if facts.function(edge.caller).is_installed_third_party() {
            continue;
        }
        let paths = shortest_paths_to_async_entries(facts, reverse, edge.caller);
        for path in paths {
            let entry_id = path.entry;
            let entry = facts.function(entry_id);
            if entry.is_sync_route
                || !entry.is_async
                || entry.is_third_party
                || matches!(entry.label, EntryLabel::Task | EntryLabel::Lifecycle)
            {
                continue;
            }
            if !include_non_production && is_non_production_entry_path(&entry.file) {
                continue;
            }

            let sink_key = format!("{RULE_SWALLOWED_CANCELLED_ERROR}:{catch_kind}");
            let (chain_steps, chain_qualnames) = build_advice_chain(
                facts,
                &path,
                edge,
                format!("`{catch_kind}` handler exits without re-raising `CancelledError`"),
                sink_key.clone(),
            );
            let chain_refs: Vec<&str> = chain_qualnames.iter().map(String::as_str).collect();
            let Some(id) = unique_site_finding_id(
                FindingIdParts {
                    rule: RULE_SWALLOWED_CANCELLED_ERROR,
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
                rule: RULE_SWALLOWED_CANCELLED_ERROR.to_string(),
                category: AdviceCategory::Correctness,
                entry: Entry {
                    file: entry.file.clone(),
                    line: entry.line,
                    name: entry.qualname.clone(),
                    label: entry.label,
                },
                chain: chain_steps,
                fix: Fix {
                    hint: "Catching `asyncio.CancelledError` (or bare `except:` / `except \
                           BaseException:` over an `await`) without re-raising swallows task \
                           cancellation and breaks `asyncio.timeout`, `wait_for`, and `TaskGroup`. \
                           Run cleanup in `finally:` or end the handler with `raise` (and use \
                           `except Exception:` for ordinary errors)."
                        .to_string(),
                    fix_group_id: fix_group_id(
                        RULE_SWALLOWED_CANCELLED_ERROR,
                        RULE_SWALLOWED_CANCELLED_ERROR,
                    ),
                },
                why: format!("async-doctor why {}:{}", edge.file, edge.line),
                docs_anchor: format!("#{RULE_SWALLOWED_CANCELLED_ERROR}"),
                docs_url: format!(
                    "https://github.com/maolonglong/async-doctor/blob/main/docs/rules.md#{RULE_SWALLOWED_CANCELLED_ERROR}"
                ),
            });
        }
    }
}
