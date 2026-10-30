//! Blocking propagation: mark sinks, BFS reverse call graph, grade findings.

mod grade;
mod higher_order;
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

use grade::{
    ParamVerdict, async_twin_hint, chain_step_for, evaluate_param_guards, label_rank,
    merge_file_io_findings, reconcile_conditions, shortest_paths_to_async_entries,
    unresolved_pattern,
};
use higher_order::materialize_higher_order;

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

/// Lookup sinks and optional async-twin hint text for a resolved external name.
pub trait SinkTable {
    fn lookup(&self, qualname: &str) -> Option<SinkHit>;
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
        skipped: facts.skipped.clone(),
        omitted_non_production_entries: 0,
        hidden_unresolved_candidates: 0,
        unresolved_summary: Vec::new(),
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
                // Awaited *async* APIs are excluded by `SinkTable::lookup` (async
                // counterparts return None). Awaited *sync* sinks still block on
                // the loop before `await` (e.g. `await redis.Redis().get(...)`)
                // and must seed. Do not blanket-skip on `is_awaited`.
                if let Some(hit) = sinks.lookup(qualname) {
                    if hit.with_only && !matches!(edge.kind, CallKind::WithEnter) {
                        continue;
                    }
                    if hit.consume_only
                        && !matches!(
                            edge.kind,
                            CallKind::GeneratorConsume | CallKind::ForIter
                        )
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

            for &edge_idx in &path.edges {
                let e = &facts.edges[edge_idx];
                let caller_fn = facts.function(e.caller);
                chain_steps.push(chain_step_for(
                    caller_fn,
                    e.file.clone(),
                    e.line,
                    e.text.clone(),
                ));
                if let CalleeRef::Project(fid) = e.callee {
                    let callee_fn = facts.function(fid);
                    chain_qualnames.push(callee_fn.full_qualname());
                }
                condition_tags.extend(e.conditions.iter().copied());
            }

            let seed_caller_fn = facts.function(seed_caller);
            chain_steps.push(chain_step_for(
                seed_caller_fn,
                seed_edge.file.clone(),
                seed_edge.line,
                seed_edge.text.clone(),
            ));
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
            // Third-party sinks under a param/branch guard (e.g. PIL `if is_path(fp):
            // open(...)`) are not provable for arbitrary caller args → candidates.
            if seed_caller_fn.is_third_party
                && seed_edge
                    .conditions
                    .iter()
                    .any(|c| c.is_branch_like() || matches!(c, ConditionTag::AsyncBranch))
            {
                force_conditional = true;
            }

            match &seed.kind {
                SeedKind::Sink(hit) => {
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
                    let why = format!("async-doctor why {}:{}", seed_edge.file, seed_edge.line);
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
                        });
                    }
                }
                SeedKind::Unresolved { reason } => {
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
                    });
                }
            }
        }
    }

    report.omitted_non_production_entries = omitted_entries.len() as u32;
    report.findings = merge_file_io_findings(raw_findings);
    report.candidates = raw_candidates;

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

    report
}
