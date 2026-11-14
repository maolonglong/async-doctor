//! Seed discovery and reverse-graph propagation for sink findings and call candidates.

use std::collections::{HashMap, HashSet};

use crate::graph::{
    AnalyzeOptions, CallKind, CalleeRef, ConditionTag, FunctionId, ParamGuard, ProjectFacts,
    is_non_production_entry_path,
};
use crate::id::{FindingIdParts, finding_id, fix_group_id};
use crate::report::{Candidate, ChainStep, Entry, Finding, Fix};

use super::grade::{
    self, ClassInitIndex, ItemSpanMap, ParamVerdict, PathToEntry, chain_step_for,
    collect_item_spans, evaluate_param_guards, reconcile_conditions, resolution_for_edge,
    shortest_paths_to_async_entries, unique_site_finding_id, unresolved_pattern,
};
use super::special::{self, special_finding_hit};
use super::{SinkHit, SinkTable};

pub(super) struct SeedDiagnostics {
    pub raw_findings: Vec<Finding>,
    pub raw_candidates: Vec<Candidate>,
    pub omitted_non_production_entries: u32,
    pub hidden_unresolved_candidates: u32,
    pub unresolved_agg: HashMap<String, (u32, HashSet<FunctionId>)>,
    pub emitted: HashSet<String>,
    pub emitted_sink_sites: HashMap<String, (FunctionId, String, u32)>,
    pub item_spans: ItemSpanMap,
}

#[derive(Clone)]
struct Seed {
    edge_idx: usize,
    kind: SeedKind,
}

#[derive(Clone)]
enum SeedKind {
    Sink(SinkHit),
    Unresolved { reason: String },
}

pub(super) fn collect_seed_diagnostics(
    facts: &ProjectFacts,
    sinks: &dyn SinkTable,
    options: AnalyzeOptions,
    reverse: &[Vec<usize>],
    class_inits: &ClassInitIndex,
) -> SeedDiagnostics {
    let seeds = collect_seeds(facts, sinks);

    // For unresolved aggregation: pattern → (sites, entries).
    let mut unresolved_agg: HashMap<String, (u32, HashSet<FunctionId>)> = HashMap::new();
    let mut emitted: HashSet<String> = HashSet::new();
    // Finding id → `(entry_id, sink_file, sink_line)` for site disambiguation.
    let mut emitted_sink_sites: HashMap<String, (FunctionId, String, u32)> = HashMap::new();
    let mut item_spans: ItemSpanMap = ItemSpanMap::new();
    let mut omitted_entries: HashSet<FunctionId> = HashSet::new();
    let mut raw_findings: Vec<Finding> = Vec::new();
    let mut raw_candidates: Vec<Candidate> = Vec::new();
    let mut hidden_unresolved_candidates = 0u32;
    let mut paths_cache: HashMap<FunctionId, Vec<PathToEntry>> = HashMap::new();
    let func_full_qualnames: Vec<String> =
        facts.functions.iter().map(|f| f.full_qualname()).collect();
    let mut edge_resolutions: Vec<Option<String>> = vec![None; facts.edges.len()];

    for seed in seeds {
        let seed_edge = &facts.edges[seed.edge_idx];
        let seed_caller = seed_edge.caller;
        let paths = paths_cache
            .entry(seed_caller)
            .or_insert_with(|| shortest_paths_to_async_entries(facts, reverse, seed_caller));
        if paths.is_empty() {
            continue;
        }
        let seed_why = format!("async-doctor why {}:{}", seed_edge.file, seed_edge.line);
        let seed_sink_meta = match &seed.kind {
            SeedKind::Sink(hit) => {
                let docs_anchor = format!("#{}", hit.rule);
                let docs_url = format!(
                    "https://github.com/maolonglong/async-doctor/blob/main/docs/rules.md{docs_anchor}"
                );
                Some((
                    format!("blocking sink `{}`", hit.sink_qualname),
                    fix_group_id(hit.rule, hit.sink_qualname),
                    docs_anchor,
                    docs_url,
                ))
            }
            SeedKind::Unresolved { .. } => None,
        };

        for path in paths {
            let entry_id = path.entry;
            let entry = facts.function(entry_id);
            if entry.is_sync_route || !entry.is_async || entry.is_third_party {
                continue;
            }

            if !options.include_non_production && is_non_production_entry_path(&entry.file) {
                omitted_entries.insert(entry_id);
                continue;
            }

            let mut chain_steps: Vec<ChainStep> = Vec::new();
            let mut chain_qualnames: Vec<String> = Vec::new();
            let mut condition_tags: Vec<ConditionTag> = Vec::new();
            let mut edge_param_guards: Vec<(FunctionId, ParamGuard)> = Vec::new();
            let mut third_party_edge_guards: Vec<(FunctionId, ParamGuard)> = Vec::new();

            // When the async entry *receives* a kwargs/partial callback, the
            // reverse path stops at that receiver and would omit the call site
            // that passed the callable. Prepend that binding hop so agents know
            // which line to change.
            for &edge_idx in &path.edges {
                let e = &facts.edges[edge_idx];
                let resolved_bind = if let Some(bi) = e.context_from_edge {
                    facts.edges.get(bi).map(|be| (bi, be))
                } else if let Some(binder) = e.context_from_caller {
                    facts.edges.iter().enumerate().find(|(_, be)| {
                        be.caller == binder
                            && matches!(be.callee, CalleeRef::Project(id) if id == e.caller)
                            && (!be.positional_callables.is_empty()
                                || !be.keyword_callables.is_empty())
                    })
                } else {
                    None
                };
                let Some((bind_idx, bind_e)) = resolved_bind else {
                    continue;
                };
                if bind_e.caller == entry_id || path.edges.contains(&bind_idx) {
                    continue;
                }
                let binder_fn = facts.function(bind_e.caller);
                let res = edge_resolutions[bind_idx]
                    .get_or_insert_with(|| resolution_for_edge(facts, bind_e))
                    .clone();
                chain_steps.push(chain_step_for(
                    binder_fn,
                    bind_e.file.clone(),
                    bind_e.line,
                    bind_e.text.clone(),
                    res,
                ));
                condition_tags.extend(bind_e.conditions.iter().copied());
            }

            for &edge_idx in &path.edges {
                let e = &facts.edges[edge_idx];
                let caller_fn = facts.function(e.caller);
                let res = edge_resolutions[edge_idx]
                    .get_or_insert_with(|| resolution_for_edge(facts, e))
                    .clone();
                chain_steps.push(chain_step_for(
                    caller_fn,
                    e.file.clone(),
                    e.line,
                    e.text.clone(),
                    res,
                ));
                if let CalleeRef::Project(fid) = e.callee {
                    chain_qualnames.push(func_full_qualnames[fid.index()].clone());
                    if grade::is_memoized(facts, fid) {
                        condition_tags.push(ConditionTag::CacheMissDependent);
                    }
                }
                condition_tags.extend(e.conditions.iter().copied());
                for &g in &e.param_guards {
                    edge_param_guards.push((e.caller, g));
                    if caller_fn.is_installed_third_party() {
                        third_party_edge_guards.push((e.caller, g));
                    }
                }
            }

            let seed_caller_fn = facts.function(seed_caller);
            // Seed hop resolution is finalized after we know Sink vs Unresolved.
            let seed_res = edge_resolutions[seed.edge_idx]
                .get_or_insert_with(|| resolution_for_edge(facts, seed_edge))
                .clone();
            let mut seed_step = chain_step_for(
                seed_caller_fn,
                seed_edge.file.clone(),
                seed_edge.line,
                seed_edge.text.clone(),
                seed_res,
            );
            condition_tags.extend(seed_edge.conditions.iter().copied());
            let seed_param_guards: Vec<(FunctionId, ParamGuard)> = seed_edge
                .param_guards
                .iter()
                .map(|&g| (seed_caller, g))
                .collect();

            let conditions = reconcile_conditions(&condition_tags);

            // Intermediate parameter guards prune provably dead multi-hop chains;
            // unknown runtime args on first-party intermediate hops remain normal
            // `async-branch` findings rather than parameter-toggled sinks.
            if evaluate_param_guards(facts, path, &edge_param_guards) == ParamVerdict::ProvablyFalse
            {
                continue;
            }
            // Parameter-controlled sink: demote unless the guarding arg is provably truthy.
            let param_verdict = evaluate_param_guards(facts, path, &seed_param_guards);
            if param_verdict == ParamVerdict::ProvablyFalse {
                continue;
            }
            let mut force_conditional = param_verdict == ParamVerdict::Unknown
                || evaluate_param_guards(facts, path, &third_party_edge_guards)
                    == ParamVerdict::Unknown;
            // Installed third-party sinks under a param/branch guard (e.g. PIL
            // `if is_path(fp): open(...)`) are not provable for arbitrary caller
            // args → candidates. Outside-editable follow-only code keeps
            // first-party grading (still never an entry).
            if seed_caller_fn.is_installed_third_party()
                && seed_edge
                    .conditions
                    .iter()
                    .any(|c| c.is_branch_like() || matches!(c, ConditionTag::AsyncBranch))
            {
                force_conditional = true;
            }
            // Nested event-loop under a proven "no running loop" guard
            // (`if loop is None`, `except RuntimeError` after get_running_loop,
            // `not loop.is_running()`, …) is runtime-safe when already async —
            // drop entirely. Other conditional nested-loop sites stay findings.
            if matches!(
                &seed.kind,
                SeedKind::Sink(h) if h.rule == special::RULE_NESTED_EVENT_LOOP
            ) && condition_tags
                .iter()
                .any(|c| matches!(c, ConditionTag::NoRunningLoopGuard))
            {
                continue;
            }

            match &seed.kind {
                SeedKind::Sink(hit) => {
                    let (seed_resolution, seed_fix_group_id, seed_docs_anchor, seed_docs_url) =
                        seed_sink_meta
                            .as_ref()
                            .expect("seed_sink_meta present for SeedKind::Sink");
                    seed_step.resolution = seed_resolution.clone();
                    // Eager evaluation is evidence, not proof that moving this
                    // operation into a worker thread is an appropriate repair.
                    if seed_edge.is_eager_offload_arg
                        || path
                            .edges
                            .iter()
                            .any(|&i| facts.edges[i].is_eager_offload_arg)
                    {
                        seed_step
                            .resolution
                            .push_str("; argument evaluated on the event loop before offload");
                    }
                    chain_steps.push(seed_step);
                    chain_qualnames.push(hit.sink_qualname.to_string());
                    let chain_refs: Vec<&str> =
                        chain_qualnames.iter().map(String::as_str).collect();
                    let Some(id) = unique_site_finding_id(
                        FindingIdParts {
                            rule: hit.rule,
                            entry_qualname: &func_full_qualnames[entry_id.index()],
                            sink_qualname: hit.sink_qualname,
                            chain_qualnames: &chain_refs,
                            occurrence: seed_edge.occurrence,
                        },
                        entry_id,
                        (&seed_edge.file, seed_edge.line),
                        &mut emitted_sink_sites,
                        &mut emitted,
                    ) else {
                        continue;
                    };

                    let entry_out = Entry {
                        file: entry.file.clone(),
                        line: entry.line,
                        name: entry.qualname.clone(),
                        label: entry.label,
                    };
                    // CLI rewrites `--path` to the actual scan root after analysis.
                    let why = seed_why.clone();
                    let docs_anchor = seed_docs_anchor.clone();
                    let docs_url = seed_docs_url.clone();
                    let fix = Fix {
                        hint: hit.fix_hint.to_string(),
                        fix_group_id: seed_fix_group_id.clone(),
                    };

                    item_spans.insert(
                        id.clone(),
                        collect_item_spans(facts, class_inits, path, seed_edge),
                    );
                    if hit.candidate_only || force_conditional {
                        let reason = if force_conditional {
                            "conditional"
                        } else {
                            hit.candidate_reason.unwrap_or("uncertain")
                        };
                        raw_candidates.push(Candidate {
                            id,
                            rule: hit.rule.to_string(),
                            entry: entry_out,
                            chain: chain_steps,
                            conditions,
                            reason: reason.to_string(),
                            fix,
                            why,
                            docs_anchor,
                            docs_url,
                        });
                    } else {
                        raw_findings.push(Finding {
                            id,
                            rule: hit.rule.to_string(),
                            entry: entry_out,
                            chain: chain_steps,
                            conditions,
                            fix,
                            why,
                            docs_anchor,
                            docs_url,
                        });
                    }
                }
                SeedKind::Unresolved { reason } => {
                    seed_step.resolution = format!("unresolved call ({reason})");
                    chain_steps.push(seed_step);
                    let pattern = unresolved_pattern(reason, &seed_edge.text);
                    let agg = unresolved_agg.entry(pattern.clone()).or_default();
                    agg.0 += 1;
                    agg.1.insert(entry_id);

                    if !options.show_unresolved {
                        hidden_unresolved_candidates += 1;
                        continue;
                    }
                    let sink_key = format!("unresolved:{reason}");
                    chain_qualnames.push(sink_key.clone());
                    let chain_refs: Vec<&str> =
                        chain_qualnames.iter().map(String::as_str).collect();
                    let id = finding_id(FindingIdParts {
                        rule: "unresolved-call",
                        entry_qualname: &func_full_qualnames[entry_id.index()],
                        sink_qualname: &sink_key,
                        chain_qualnames: &chain_refs,
                        occurrence: seed_edge.occurrence,
                    });
                    if !emitted.insert(id.clone()) {
                        continue;
                    }
                    item_spans.insert(
                        id.clone(),
                        collect_item_spans(facts, class_inits, path, seed_edge),
                    );
                    raw_candidates.push(Candidate {
                        id,
                        rule: "unresolved-call".to_string(),
                        entry: Entry {
                            file: entry.file.clone(),
                            line: entry.line,
                            name: entry.qualname.clone(),
                            label: entry.label,
                        },
                        chain: chain_steps,
                        conditions,
                        reason: reason.clone(),
                        fix: Fix {
                            hint: String::new(),
                            fix_group_id: fix_group_id("unresolved-call", &sink_key),
                        },
                        why: seed_why.clone(),
                        docs_anchor: "#unresolved-call".into(),
                        docs_url: "https://github.com/maolonglong/async-doctor/blob/main/docs/rules.md#unresolved-call".into(),
                    });
                }
            }
        }
    }

    SeedDiagnostics {
        raw_findings,
        raw_candidates,
        omitted_non_production_entries: omitted_entries.len() as u32,
        hidden_unresolved_candidates,
        unresolved_agg,
        emitted,
        emitted_sink_sites,
        item_spans,
    }
}

fn collect_seeds(facts: &ProjectFacts, sinks: &dyn SinkTable) -> Vec<Seed> {
    let mut seeds = Vec::new();
    for (idx, edge) in facts.edges.iter().enumerate() {
        if edge.is_offload_wrapper {
            continue;
        }
        match &edge.callee {
            CalleeRef::External { qualname } => {
                // Structural: a resolved `async def` callee never seeds as sync IO,
                // even when its External qualname looks like a sync sink family
                // (subclass / wrapper of AsyncClient, etc.).
                if edge.callee_is_coroutine == Some(true) {
                    continue;
                }
                // Nested event-loop APIs and first-party async anti-patterns are
                // correctness errors when reachable from async without an offload cut.
                if let Some(hit) = special_finding_hit(qualname, facts.function(edge.caller)) {
                    if hit.with_only && !matches!(edge.kind, CallKind::WithEnter) {
                        continue;
                    }
                    if hit.consume_only
                        && !matches!(edge.kind, CallKind::GeneratorConsume | CallKind::ForIter)
                    {
                        continue;
                    }
                    seeds.push(Seed {
                        edge_idx: idx,
                        kind: SeedKind::Sink(hit),
                    });
                    continue;
                }
                // Name-based async-counterpart rules are only a fallback when
                // ty could not resolve coroutine-ness. Awaited *sync* defs
                // (e.g. `await redis.Redis().get(...)`) still seed.
                if let Some(mut hit) = sinks.lookup(qualname) {
                    if grade::is_cheap_path_op(qualname, edge) {
                        hit.candidate_only = true;
                        hit.candidate_reason = Some("cheap-path-op");
                    }
                    if hit.with_only && !matches!(edge.kind, CallKind::WithEnter) {
                        continue;
                    }
                    if hit.consume_only
                        && !matches!(edge.kind, CallKind::GeneratorConsume | CallKind::ForIter)
                    {
                        continue;
                    }
                    seeds.push(Seed {
                        edge_idx: idx,
                        kind: SeedKind::Sink(hit),
                    });
                }
            }
            CalleeRef::Unresolved { reason } => {
                seeds.push(Seed {
                    edge_idx: idx,
                    kind: SeedKind::Unresolved {
                        reason: reason.clone(),
                    },
                });
            }
            CalleeRef::Project(_) | CalleeRef::Offload(_) | CalleeRef::ParamInvoke { .. } => {}
        }
    }
    seeds
}
