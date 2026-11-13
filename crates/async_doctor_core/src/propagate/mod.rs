//! Blocking propagation: mark sinks, BFS reverse call graph, grade findings.

mod advice;
mod anti_pattern_advice;
mod grade;
mod higher_order;
mod special;
#[cfg(test)]
mod tests;

use std::collections::{HashMap, HashSet};

use crate::graph::{
    AnalyzeOptions, CallKind, CalleeRef, ConditionTag, FunctionId, ParamGuard, ProjectFacts,
    is_non_production_entry_path,
};
use crate::id::{FindingIdParts, finding_id, fix_group_id};
use crate::report::{
    Candidate, ChainStep, Entry, Finding, Fix, Report, SCHEMA_VERSION, UnresolvedPattern,
};

use advice::collect_offload_twin_advice;
use grade::{
    ParamVerdict, PathToEntry, async_twin_hint, chain_step_for, evaluate_param_guards, label_rank,
    merge_file_io_findings, merge_same_line_ctor_call_findings, reconcile_conditions,
    resolution_for_edge, shortest_paths_to_async_entries, unique_site_finding_id,
    unresolved_pattern,
};
use higher_order::{materialize_higher_order, materialize_higher_order_owned};
use special::{
    collect_eager_offload_findings, collect_offload_coroutine_findings, special_finding_hit,
};

#[derive(Debug, Clone, Copy)]
pub struct SinkHit {
    pub rule: &'static str,
    pub sink_qualname: &'static str,
    pub fix_hint: &'static str,
    /// When true, emit a candidate instead of a finding (e.g. small metadata IO).
    pub candidate_only: bool,
    pub candidate_reason: Option<&'static str>,
    /// When true, only fire for `with` / context-manager enter edges (e.g. Engine.begin).
    pub with_only: bool,
    /// When true, only fire when the call is consumed as a generator/iterator
    /// (e.g. `for msg in pubsub.listen()` / comprehension over `listen()`).
    pub consume_only: bool,
}

/// Confidence for a structured async-twin table entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TwinConfidence {
    High,
}

/// High-confidence async twin for a sync sink (advice tier).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AsyncTwinHit {
    pub symbol: &'static str,
    pub same_package: bool,
    pub needs: &'static [&'static str],
    pub any_needs: &'static [&'static str],
    pub confidence: TwinConfidence,
}

/// Lookup sinks and optional structured async twins for a resolved external name.
pub trait SinkTable {
    fn lookup(&self, qualname: &str) -> Option<SinkHit>;

    /// Structured high-confidence twin for `offload-has-async-twin` advice.
    /// Default: none (tests / minimal tables).
    fn async_twin(&self, _qualname: &str) -> Option<AsyncTwinHit> {
        None
    }
}

/// Analyze with default options (skip non-production entries; hide unresolved).
pub fn analyze(facts: &ProjectFacts, sinks: &dyn SinkTable) -> Report {
    analyze_with_options(facts, sinks, AnalyzeOptions::default())
}

/// Analyze project facts against a sink table and produce a report.
pub fn analyze_with_options(
    facts: &ProjectFacts,
    sinks: &dyn SinkTable,
    options: AnalyzeOptions,
) -> Report {
    let owned = materialize_higher_order(facts);
    analyze_materialized(&owned, sinks, options)
}

/// Analyze owned project facts without cloning [`ProjectFacts`] during higher-order materialization.
pub fn analyze_owned(
    facts: ProjectFacts,
    sinks: &dyn SinkTable,
    options: AnalyzeOptions,
) -> Report {
    let materialized = materialize_higher_order_owned(facts);
    analyze_materialized(&materialized, sinks, options)
}

fn analyze_materialized(
    facts: &ProjectFacts,
    sinks: &dyn SinkTable,
    options: AnalyzeOptions,
) -> Report {
    let mut first_party_files = HashSet::new();
    let mut async_entry_count = 0u32;
    for f in &facts.functions {
        if f.is_third_party {
            continue;
        }
        if !f.file.is_empty() {
            first_party_files.insert(f.file.as_str());
        }
        if f.is_async
            && !f.is_sync_route
            && (options.include_non_production || !is_non_production_entry_path(&f.file))
        {
            async_entry_count += 1;
        }
    }

    let file_count = if facts.no_python_files {
        0
    } else {
        first_party_files.len().max(1) as u32
    };

    let mut report = Report {
        schema_version: SCHEMA_VERSION,
        complete: facts.complete,
        score: 100,
        environment: facts.environment.clone(),
        findings: Vec::new(),
        candidates: Vec::new(),
        advice: Vec::new(),
        location_less_candidates: Vec::new(),
        skipped: facts.skipped.clone(),
        message: if facts.no_python_files {
            Some("No Python files were found.".to_string())
        } else {
            None
        },
        omitted_non_production_entries: 0,
        hidden_unresolved_candidates: 0,
        omitted_findings: 0,
        omitted_candidates: 0,
        omitted_advice: 0,
        unresolved_summary: Vec::new(),
        unused_suppressions: Vec::new(),
        sink_sites: Vec::new(),
        file_count,
        async_entry_count,
    };

    // Dense vec + sorted preds so reverse-graph walk order is stable across
    // cold extract vs warm cache materialization (edge insertion order differs).
    let mut reverse: Vec<Vec<usize>> = vec![Vec::new(); facts.functions.len()];
    for (idx, edge) in facts.edges.iter().enumerate() {
        if edge.is_offload_wrapper {
            continue;
        }
        if let CalleeRef::Project(callee) = edge.callee
            && let Some(slot) = reverse.get_mut(callee.index())
        {
            slot.push(idx);
        }
    }
    for idxs in &mut reverse {
        idxs.sort_by(|&a, &b| {
            let ea = &facts.edges[a];
            let eb = &facts.edges[b];
            (ea.line, ea.occurrence, ea.text.as_str(), a).cmp(&(
                eb.line,
                eb.occurrence,
                eb.text.as_str(),
                b,
            ))
        });
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
                let caller_is_installed_tp = facts.function(edge.caller).is_installed_third_party();
                if let Some(hit) = special_finding_hit(qualname, caller_is_installed_tp) {
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

    // For unresolved aggregation: pattern → (sites, entries).
    let mut unresolved_agg: HashMap<String, (u32, HashSet<FunctionId>)> = HashMap::new();

    let mut emitted: HashSet<String> = HashSet::new();
    // Finding id → `(entry_id, sink_file, sink_line)` for site disambiguation.
    let mut emitted_sink_sites: HashMap<String, (FunctionId, String, u32)> = HashMap::new();
    let mut omitted_entries: HashSet<FunctionId> = HashSet::new();
    let mut raw_findings: Vec<Finding> = Vec::new();
    let mut raw_candidates: Vec<Candidate> = Vec::new();
    let mut paths_cache: HashMap<FunctionId, Vec<PathToEntry>> = HashMap::new();
    let func_full_qualnames: Vec<String> =
        facts.functions.iter().map(|f| f.full_qualname()).collect();
    let mut edge_resolutions: Vec<Option<String>> = vec![None; facts.edges.len()];

    for seed in seeds {
        let seed_edge = &facts.edges[seed.edge_idx];
        let seed_caller = seed_edge.caller;
        let paths = paths_cache
            .entry(seed_caller)
            .or_insert_with(|| shortest_paths_to_async_entries(facts, &reverse, seed_caller));
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
                if let Some(g) = e.param_guard {
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
            let mut seed_param_guards: Vec<(FunctionId, ParamGuard)> = Vec::new();
            if let Some(g) = seed_edge.param_guard {
                seed_param_guards.push((seed_caller, g));
            }

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

                    let mut hint = hit.fix_hint.to_string();
                    // Eager offload may mark the edge into the sink's caller
                    // (`to_thread(work())` → work → sleep) rather than the sink edge.
                    let eager_offload = seed_edge.is_eager_offload_arg
                        || path
                            .edges
                            .iter()
                            .any(|&i| facts.edges[i].is_eager_offload_arg);
                    if eager_offload {
                        hint = format!(
                            "{hint} This call runs on the event loop before offload — \
                             pass the callable (e.g. `asyncio.to_thread(f, ...)`) \
                             rather than calling it (`to_thread(f())`)."
                        );
                    }
                    if let Some(twin) = async_twin_hint(facts, entry_id, path) {
                        hint = format!("{hint} Async twin available: `{twin}`.");
                    }

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
                        hint,
                        fix_group_id: seed_fix_group_id.clone(),
                    };

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
                        report.hidden_unresolved_candidates += 1;
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
                            hint: "Could not resolve call target; inspect dynamically.".to_string(),
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

    // Failed first-party imports: once per (file, module) — already deduped.
    for imp in &facts.unresolved_imports {
        let pattern = format!("unresolved-import:{}", imp.module);
        let agg = unresolved_agg.entry(pattern.clone()).or_default();
        agg.0 += 1;
        if !options.show_unresolved {
            report.hidden_unresolved_candidates += 1;
            continue;
        }
        let sink_key = pattern.clone();
        let id = finding_id(FindingIdParts {
            rule: "unresolved-import",
            entry_qualname: &format!("{}:{}", imp.file, imp.module),
            sink_qualname: &sink_key,
            chain_qualnames: &[&sink_key],
            occurrence: 0,
        });
        if !emitted.insert(id.clone()) {
            continue;
        }
        raw_candidates.push(Candidate {
            id,
            rule: "unresolved-import".to_string(),
            entry: Entry {
                file: imp.file.clone(),
                line: imp.line,
                name: imp.module.clone(),
                label: crate::report::EntryLabel::Other,
            },
            chain: vec![ChainStep::project(
                imp.file.clone(),
                imp.line,
                imp.text.clone(),
            )
            .with_resolution(format!("unresolved import `{}`", imp.module))],
            conditions: Vec::new(),
            reason: format!("unresolved import `{}`", imp.module),
            fix: Fix {
                hint: "Could not resolve import; check package layout / editable install."
                    .to_string(),
                fix_group_id: fix_group_id("unresolved-import", &sink_key),
            },
            why: format!("async-doctor why {}:{}", imp.file, imp.line),
            docs_anchor: "#unresolved-call".into(),
            docs_url: "https://github.com/maolonglong/async-doctor/blob/main/docs/rules.md#unresolved-call".into(),
        });
    }

    // Missing third-party packages: one location-less note (never per-import).
    // Always under `locationLessCandidates` (not hidden candidates). Empty `why`
    // — there is no file:line locator for an environment-level aggregate.
    let n_tp = facts.unresolved_third_party_modules.len() as u32;
    let n_resolved = facts.resolved_third_party_modules.len() as u32;
    report.environment.unresolved_third_party = n_tp;
    report.environment.resolved_third_party = n_resolved;
    report.environment.refresh_note();
    if n_tp > 0 {
        let pattern = crate::environment::ScanEnvironment::missing_third_party_reason(
            n_tp,
            report.environment.kind,
        );
        let agg = unresolved_agg.entry(pattern.clone()).or_default();
        agg.0 = n_tp;
        let sink_key = "unresolved-import:third-party-not-installed";
        let id = finding_id(FindingIdParts {
            rule: "unresolved-import",
            entry_qualname: "third-party-not-installed",
            sink_qualname: sink_key,
            chain_qualnames: &[sink_key],
            occurrence: 0,
        });
        if emitted.insert(id.clone()) {
            report.location_less_candidates.push(Candidate {
                id,
                rule: "unresolved-import".to_string(),
                entry: Entry {
                    file: String::new(),
                    line: 0,
                    name: "third-party-not-installed".into(),
                    label: crate::report::EntryLabel::Other,
                },
                chain: Vec::new(),
                conditions: Vec::new(),
                reason: pattern,
                fix: Fix {
                    hint: "Install project dependencies into the active venv, then re-scan."
                        .to_string(),
                    fix_group_id: fix_group_id("unresolved-import", sink_key),
                },
                why: String::new(),
                docs_anchor: "#unresolved-call".into(),
                docs_url: "https://github.com/maolonglong/async-doctor/blob/main/docs/rules.md#unresolved-call".into(),
            });
        }
    }

    report.omitted_non_production_entries = omitted_entries.len() as u32;
    report.findings = merge_same_line_ctor_call_findings(merge_file_io_findings(raw_findings));
    // offload-coroutine findings (error) — additive; never suppresses existing rules.
    report.findings.extend(collect_offload_coroutine_findings(
        facts,
        &reverse,
        options.include_non_production,
        &mut emitted_sink_sites,
        &mut emitted,
    ));
    let eager_offload_findings = collect_eager_offload_findings(
        facts,
        &reverse,
        options.include_non_production,
        &report.findings,
        &mut emitted_sink_sites,
        &mut emitted,
    );
    report.findings.extend(eager_offload_findings);
    // Empty entry.file/chain belong in locationLessCandidates, not candidates.
    let (located, location_less): (Vec<_>, Vec<_>) = raw_candidates
        .into_iter()
        .partition(|c| !(c.entry.file.is_empty() && c.chain.is_empty()));
    report.candidates = located;
    report.location_less_candidates.extend(location_less);
    report.advice =
        collect_offload_twin_advice(facts, sinks, &reverse, options.include_non_production);

    if options.show_unresolved {
        let mut summary: Vec<UnresolvedPattern> = unresolved_agg
            .into_iter()
            .map(|(pattern, (sites, entries))| UnresolvedPattern {
                pattern,
                entries_affected: entries.len() as u32,
                sites,
            })
            .collect();
        summary.sort_by(|a, b| {
            b.entries_affected
                .cmp(&a.entries_affected)
                .then_with(|| b.sites.cmp(&a.sites))
                .then_with(|| a.pattern.cmp(&b.pattern))
        });
        report.unresolved_summary = summary;
    }

    report.findings.sort_by(|a, b| {
        (
            label_rank(a.entry.label),
            a.entry.file.as_str(),
            a.entry.line,
            a.chain.first().map(|c| c.line).unwrap_or(0),
            a.id.as_str(),
        )
            .cmp(&(
                label_rank(b.entry.label),
                b.entry.file.as_str(),
                b.entry.line,
                b.chain.first().map(|c| c.line).unwrap_or(0),
                b.id.as_str(),
            ))
    });
    report.candidates.sort_by(|a, b| {
        (a.entry.file.as_str(), a.entry.line, a.id.as_str()).cmp(&(
            b.entry.file.as_str(),
            b.entry.line,
            b.id.as_str(),
        ))
    });
    // Keep advice ordered by offload site file/line (set in collect_offload_twin_advice).
    report.advice.sort_by(|a, b| {
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

    report.refresh_sink_sites();
    report
}
