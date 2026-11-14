//! Blocking propagation: mark sinks, BFS reverse call graph, grade findings.

mod advice;
mod anti_pattern_advice;
mod findings;
mod grade;
mod higher_order;
mod special;
mod sync_callers;
#[cfg(test)]
mod tests;

use std::collections::HashSet;

use crate::graph::{AnalyzeOptions, CalleeRef, ProjectFacts, is_non_production_entry_path};
use crate::id::{FindingIdParts, finding_id, fix_group_id};
use crate::report::{Candidate, ChainStep, Entry, Fix, Report, SCHEMA_VERSION, UnresolvedPattern};

use advice::collect_offload_twin_advice;
use findings::collect_seed_diagnostics;
use grade::{
    build_class_inits, label_rank, merge_file_io_findings, merge_same_line_ctor_call_findings,
};
use higher_order::{materialize_higher_order, materialize_higher_order_owned};
use special::{
    SpecialFindingCtx, collect_eager_offload_findings, collect_offload_coroutine_findings,
};
use sync_callers::SyncCallerIndex;

#[derive(Debug, Clone, Copy)]
pub struct SinkHit {
    pub rule: &'static str,
    pub sink_qualname: &'static str,
    /// A proven local correction, or empty when no repair is established.
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

/// Resolvable async API associated with a sync sink (advice tier).
/// Availability does not establish equivalent behavior or a suitable migration.
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

    /// Structured async-API availability for `offload-has-async-twin` advice.
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
        ignore_candidate_files: None,
        item_spans: std::collections::BTreeMap::new(),
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

    let class_inits = build_class_inits(facts);
    let mut seed_diag = collect_seed_diagnostics(facts, sinks, options, &reverse, &class_inits);
    report.omitted_non_production_entries = seed_diag.omitted_non_production_entries;
    report.hidden_unresolved_candidates = seed_diag.hidden_unresolved_candidates;

    // Failed first-party imports: once per (file, module) — already deduped.
    for imp in &facts.unresolved_imports {
        let pattern = format!("unresolved-import:{}", imp.module);
        let agg = seed_diag.unresolved_agg.entry(pattern.clone()).or_default();
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
        if !seed_diag.emitted.insert(id.clone()) {
            continue;
        }
        seed_diag.raw_candidates.push(Candidate {
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
                hint: String::new(),
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
    let mut unresolved_tp_names = facts.unresolved_third_party_modules.clone();
    unresolved_tp_names.sort();
    unresolved_tp_names.dedup();
    unresolved_tp_names
        .truncate(crate::environment::ScanEnvironment::MAX_UNRESOLVED_THIRD_PARTY_MODULES);
    report.environment.unresolved_third_party = n_tp;
    report.environment.resolved_third_party = n_resolved;
    report.environment.unresolved_third_party_modules = unresolved_tp_names;
    report.environment.refresh_note();
    if n_tp > 0 {
        let pattern = crate::environment::ScanEnvironment::missing_third_party_reason(
            n_tp,
            report.environment.kind,
        );
        let agg = seed_diag.unresolved_agg.entry(pattern.clone()).or_default();
        agg.0 = n_tp;
        let sink_key = "unresolved-import:third-party-not-installed";
        let id = finding_id(FindingIdParts {
            rule: "unresolved-import",
            entry_qualname: "third-party-not-installed",
            sink_qualname: sink_key,
            chain_qualnames: &[sink_key],
            occurrence: 0,
        });
        if seed_diag.emitted.insert(id.clone()) {
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
                    hint: String::new(),
                    fix_group_id: fix_group_id("unresolved-import", sink_key),
                },
                why: String::new(),
                docs_anchor: "#unresolved-call".into(),
                docs_url: "https://github.com/maolonglong/async-doctor/blob/main/docs/rules.md#unresolved-call".into(),
            });
        }
    }

    let merged_io = merge_file_io_findings(seed_diag.raw_findings, &mut seed_diag.item_spans);
    report.findings = merge_same_line_ctor_call_findings(merged_io, &mut seed_diag.item_spans);
    // offload-coroutine and eager-offload-call findings (error) — additive; never suppress existing rules.
    let mut special_ctx = SpecialFindingCtx {
        facts,
        reverse: &reverse,
        include_non_production: options.include_non_production,
        class_inits: &class_inits,
        item_spans: &mut seed_diag.item_spans,
        emitted_sites: &mut seed_diag.emitted_sink_sites,
        emitted: &mut seed_diag.emitted,
    };
    report
        .findings
        .extend(collect_offload_coroutine_findings(&mut special_ctx));
    let eager_offload_findings = collect_eager_offload_findings(&mut special_ctx, &report.findings);
    report.findings.extend(eager_offload_findings);
    // Empty entry.file/chain belong in locationLessCandidates, not candidates.
    let (located, location_less): (Vec<_>, Vec<_>) = seed_diag
        .raw_candidates
        .into_iter()
        .partition(|c| !(c.entry.file.is_empty() && c.chain.is_empty()));
    report.candidates = located;
    report.location_less_candidates.extend(location_less);
    let sync_callers = SyncCallerIndex::new(facts);
    report.advice = collect_offload_twin_advice(
        facts,
        sinks,
        &reverse,
        &sync_callers,
        options.include_non_production,
        &class_inits,
        &mut seed_diag.item_spans,
    );
    report.item_spans = seed_diag.item_spans;

    if options.show_unresolved {
        let mut summary: Vec<UnresolvedPattern> = seed_diag
            .unresolved_agg
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

    report.sink_sites = sync_callers.summarize_sink_sites(facts, &report.findings);
    report.refresh_score();
    report
}
