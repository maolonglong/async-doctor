//! Blocking propagation: mark sinks, BFS reverse call graph, grade findings.

mod advice;
mod grade;
mod higher_order;
mod special;
#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, HashMap, HashSet};

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
    ParamVerdict, async_twin_hint, chain_step_for, evaluate_param_guards, label_rank,
    merge_file_io_findings, reconcile_conditions, resolution_for_edge,
    shortest_paths_to_async_entries, unresolved_pattern,
};
use higher_order::materialize_higher_order;
use special::{collect_offload_coroutine_findings, nested_event_loop_hit};

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
    let facts = &owned;

    let mut report = Report {
        schema_version: SCHEMA_VERSION,
        complete: facts.complete,
        findings: Vec::new(),
        candidates: Vec::new(),
        advice: Vec::new(),
        skipped: facts.skipped.clone(),
        omitted_non_production_entries: 0,
        hidden_unresolved_candidates: 0,
        unresolved_summary: Vec::new(),
        unused_suppressions: Vec::new(),
        sink_sites: Vec::new(),
    };

    // BTreeMap + sorted preds so reverse-graph walk order is stable across
    // cold extract vs warm cache materialization (edge insertion order differs).
    let mut reverse: BTreeMap<FunctionId, Vec<usize>> = BTreeMap::new();
    for (idx, edge) in facts.edges.iter().enumerate() {
        if edge.is_offload_wrapper {
            continue;
        }
        if let CalleeRef::Project(callee) = edge.callee {
            reverse.entry(callee).or_default().push(idx);
        }
    }
    for idxs in reverse.values_mut() {
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
                // Nested event-loop APIs are correctness errors when reachable
                // from async without an offload cut (reverse BFS already skips
                // offload edges, so behind-offload sites stay silent).
                if let Some(hit) = nested_event_loop_hit(qualname) {
                    seeds.push(Seed {
                        edge_idx: idx,
                        kind: SeedKind::Sink(hit),
                    });
                    continue;
                }
                // Name-based async-counterpart rules are only a fallback when
                // ty could not resolve coroutine-ness. Awaited *sync* defs
                // (e.g. `await redis.Redis().get(...)`) still seed.
                if let Some(hit) = sinks.lookup(qualname) {
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
    let mut omitted_entries: HashSet<FunctionId> = HashSet::new();
    let mut raw_findings: Vec<Finding> = Vec::new();
    let mut raw_candidates: Vec<Candidate> = Vec::new();

    for seed in seeds {
        let seed_edge = &facts.edges[seed.edge_idx];
        let seed_caller = seed_edge.caller;
        let paths = shortest_paths_to_async_entries(facts, &reverse, seed_caller);

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
            let mut param_guards: Vec<(FunctionId, ParamGuard)> = Vec::new();

            // When the async entry *receives* a kwargs/partial callback, the
            // reverse path stops at that receiver and would omit the call site
            // that passed the callable. Prepend that binding hop so agents know
            // which line to change.
            for &edge_idx in &path.edges {
                let e = &facts.edges[edge_idx];
                let Some(binder) = e.context_from_caller else {
                    continue;
                };
                if binder == entry_id {
                    continue;
                }
                let already = path.edges.iter().any(|&i| {
                    let be = &facts.edges[i];
                    be.caller == binder
                        && matches!(be.callee, CalleeRef::Project(id) if id == e.caller)
                });
                if already {
                    continue;
                }
                let Some(bind_e) = facts.edges.iter().find(|be| {
                    be.caller == binder
                        && matches!(be.callee, CalleeRef::Project(id) if id == e.caller)
                        && (!be.positional_callables.is_empty() || !be.keyword_callables.is_empty())
                }) else {
                    continue;
                };
                let binder_fn = facts.function(binder);
                chain_steps.push(chain_step_for(
                    binder_fn,
                    bind_e.file.clone(),
                    bind_e.line,
                    bind_e.text.clone(),
                    resolution_for_edge(facts, bind_e),
                ));
                condition_tags.extend(bind_e.conditions.iter().copied());
            }

            for &edge_idx in &path.edges {
                let e = &facts.edges[edge_idx];
                let caller_fn = facts.function(e.caller);
                chain_steps.push(chain_step_for(
                    caller_fn,
                    e.file.clone(),
                    e.line,
                    e.text.clone(),
                    resolution_for_edge(facts, e),
                ));
                if let CalleeRef::Project(fid) = e.callee {
                    let callee_fn = facts.function(fid);
                    chain_qualnames.push(callee_fn.full_qualname());
                }
                condition_tags.extend(e.conditions.iter().copied());
            }

            let seed_caller_fn = facts.function(seed_caller);
            // Seed hop resolution is finalized after we know Sink vs Unresolved.
            let mut seed_step = chain_step_for(
                seed_caller_fn,
                seed_edge.file.clone(),
                seed_edge.line,
                seed_edge.text.clone(),
                resolution_for_edge(facts, seed_edge),
            );
            condition_tags.extend(seed_edge.conditions.iter().copied());
            if let Some(g) = seed_edge.param_guard {
                param_guards.push((seed_caller, g));
            }

            let conditions = reconcile_conditions(&condition_tags);

            // Parameter-controlled sink: demote unless the guarding arg is provably truthy.
            let param_verdict = evaluate_param_guards(facts, &path, &param_guards);
            if param_verdict == ParamVerdict::ProvablyFalse {
                continue;
            }
            let mut force_conditional = param_verdict == ParamVerdict::Unknown;
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
            // Nested event-loop APIs behind `if loop is None` / similar guards
            // (litellm `_dispatch_from_sync`) are runtime-safe when a loop is
            // already running — demote to candidates to keep noise low.
            if matches!(
                &seed.kind,
                SeedKind::Sink(h) if h.rule == special::RULE_NESTED_EVENT_LOOP
            ) && (seed_edge
                .conditions
                .iter()
                .any(|c| c.is_branch_like() || matches!(c, ConditionTag::AsyncBranch))
                || path.edges.iter().any(|&i| {
                    facts.edges[i]
                        .conditions
                        .iter()
                        .any(|c| c.is_branch_like() || matches!(c, ConditionTag::AsyncBranch))
                }))
            {
                force_conditional = true;
            }

            match &seed.kind {
                SeedKind::Sink(hit) => {
                    seed_step.resolution = format!("blocking sink `{}`", hit.sink_qualname);
                    chain_steps.push(seed_step);
                    chain_qualnames.push(hit.sink_qualname.to_string());
                    let chain_refs: Vec<&str> =
                        chain_qualnames.iter().map(String::as_str).collect();
                    let id = finding_id(FindingIdParts {
                        rule: hit.rule,
                        entry_qualname: &entry.full_qualname(),
                        sink_qualname: hit.sink_qualname,
                        chain_qualnames: &chain_refs,
                        occurrence: seed_edge.occurrence,
                    });
                    if !emitted.insert(id.clone()) {
                        continue;
                    }

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
                    if let Some(twin) = async_twin_hint(facts, entry_id, &path) {
                        hint = format!("{hint} Async twin available: `{twin}`.");
                    }

                    let entry_out = Entry {
                        file: entry.file.clone(),
                        line: entry.line,
                        name: entry.qualname.clone(),
                        label: entry.label,
                    };
                    // CLI rewrites `--path` to the actual scan root after analysis.
                    let why = format!("async-doctor why {}:{}", seed_edge.file, seed_edge.line);
                    let docs_anchor = format!("#{}", hit.rule);
                    let docs_url = format!(
                        "https://github.com/maolonglong/async-doctor/blob/main/docs/rules.md{docs_anchor}"
                    );
                    let fix = Fix {
                        hint,
                        fix_group_id: fix_group_id(hit.rule, hit.sink_qualname),
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
                        entry_qualname: &entry.full_qualname(),
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
                        why: format!("async-doctor why {}:{}", seed_edge.file, seed_edge.line),
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

    // Missing third-party packages: one aggregated note (never per-import).
    let n_tp = facts.unresolved_third_party_modules.len();
    if n_tp > 0 {
        let pattern = format!("{n_tp} third-party modules not installed; run in the project venv");
        let agg = unresolved_agg.entry(pattern.clone()).or_default();
        agg.0 = n_tp as u32;
        if !options.show_unresolved {
            report.hidden_unresolved_candidates += 1;
        } else {
            let sink_key = "unresolved-import:third-party-not-installed";
            let id = finding_id(FindingIdParts {
                rule: "unresolved-import",
                entry_qualname: "third-party-not-installed",
                sink_qualname: sink_key,
                chain_qualnames: &[sink_key],
                occurrence: 0,
            });
            if emitted.insert(id.clone()) {
                raw_candidates.push(Candidate {
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
                    why: "async-doctor --show-unresolved".into(),
                    docs_anchor: "#unresolved-call".into(),
                    docs_url: "https://github.com/maolonglong/async-doctor/blob/main/docs/rules.md#unresolved-call".into(),
                });
            }
        }
    }

    report.omitted_non_production_entries = omitted_entries.len() as u32;
    report.findings = merge_file_io_findings(raw_findings);
    // offload-coroutine findings (error) — additive; never suppresses existing rules.
    report.findings.extend(collect_offload_coroutine_findings(
        facts,
        &reverse,
        options.include_non_production,
        &mut emitted,
    ));
    report.candidates = raw_candidates;
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
    report.advice.sort_by(|a, b| {
        (
            a.fix.fix_group_id.as_str(),
            a.entry.file.as_str(),
            a.entry.line,
            a.id.as_str(),
        )
            .cmp(&(
                b.fix.fix_group_id.as_str(),
                b.entry.file.as_str(),
                b.entry.line,
                b.id.as_str(),
            ))
    });

    report.refresh_sink_sites();
    report
}
