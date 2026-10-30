//! Blocking propagation: mark sinks, BFS reverse call graph, grade findings.

use std::collections::{HashMap, HashSet, VecDeque};

use crate::graph::{
    AnalyzeOptions, CalleeRef, ConditionTag, FunctionId, LiteralArg, ParamGuard, ProjectFacts,
    is_non_production_entry_path,
};
use crate::id::{FindingIdParts, finding_id, fix_group_id};
use crate::report::{
    Candidate, ChainStep, Entry, EntryLabel, Finding, Fix, Report, SCHEMA_VERSION,
};

/// Rule metadata looked up for an external sink qualified name.
#[derive(Debug, Clone, Copy)]
pub struct SinkHit {
    pub rule: &'static str,
    pub sink_qualname: &'static str,
    pub fix_hint: &'static str,
    /// When true, emit a candidate instead of a finding (e.g. small metadata IO).
    pub candidate_only: bool,
    pub candidate_reason: Option<&'static str>,
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
    let mut report = Report {
        schema_version: SCHEMA_VERSION,
        complete: facts.complete,
        findings: Vec::new(),
        candidates: Vec::new(),
        skipped: facts.skipped.clone(),
        omitted_non_production_entries: 0,
        hidden_unresolved_candidates: 0,
    };

    let mut reverse: HashMap<FunctionId, Vec<usize>> = HashMap::new();
    for (idx, edge) in facts.edges.iter().enumerate() {
        if edge.is_offload_wrapper {
            continue;
        }
        if let CalleeRef::Project(callee) = edge.callee {
            reverse.entry(callee).or_default().push(idx);
        }
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
                if let Some(hit) = sinks.lookup(qualname) {
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
            CalleeRef::Project(_) | CalleeRef::Offload(_) => {}
        }
    }

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
            if entry.is_sync_route || !entry.is_async {
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
                chain_steps.push(ChainStep {
                    file: e.file.clone(),
                    line: e.line,
                    text: e.text.clone(),
                });
                if let CalleeRef::Project(fid) = e.callee {
                    chain_qualnames.push(facts.function(fid).full_qualname());
                }
                condition_tags.extend(e.conditions.iter().copied());
            }

            chain_steps.push(ChainStep {
                file: seed_edge.file.clone(),
                line: seed_edge.line,
                text: seed_edge.text.clone(),
            });
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
            let force_conditional = param_verdict == ParamVerdict::Unknown;

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

fn label_rank(label: EntryLabel) -> u8 {
    match label {
        EntryLabel::Route => 0,
        EntryLabel::Lifecycle => 1,
        EntryLabel::Task => 2,
        EntryLabel::Other => 3,
    }
}

/// Merge hop tags into a coherent list: never keep unconditional/all-paths
/// alongside any branch-like tag.
fn reconcile_conditions(tags: &[ConditionTag]) -> Vec<String> {
    let has_branch = tags.iter().any(|t| t.is_branch_like());
    let has_loop = tags.contains(&ConditionTag::Loop);
    let has_cache = tags.contains(&ConditionTag::CacheMissDependent);
    let has_async_branch = tags.contains(&ConditionTag::AsyncBranch);
    let mut out = Vec::new();
    if has_loop {
        out.push(ConditionTag::Loop.as_str().to_string());
    }
    if has_cache {
        out.push(ConditionTag::CacheMissDependent.as_str().to_string());
    }
    // Emit async-branch for if/else/try/match (and when both cache + other branch tags appear).
    if has_async_branch || (has_branch && !has_cache) {
        out.push(ConditionTag::AsyncBranch.as_str().to_string());
    }
    if !has_branch && !has_loop {
        if tags.contains(&ConditionTag::Unconditional) {
            out.push(ConditionTag::Unconditional.as_str().to_string());
        }
        if tags.contains(&ConditionTag::AllPaths) {
            out.push(ConditionTag::AllPaths.as_str().to_string());
        }
        if out.is_empty() {
            out.push(ConditionTag::Unconditional.as_str().to_string());
            out.push(ConditionTag::AllPaths.as_str().to_string());
        }
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ParamVerdict {
    ProvablyTrue,
    ProvablyFalse,
    Unknown,
}

fn evaluate_param_guards(
    facts: &ProjectFacts,
    path: &PathToEntry,
    guards: &[(FunctionId, ParamGuard)],
) -> ParamVerdict {
    if guards.is_empty() {
        return ParamVerdict::ProvablyTrue;
    }
    let mut any_unknown = false;
    for &(fn_id, guard) in guards {
        // Find the edge on the path (or direct) that calls into fn_id.
        let call_edge = path.edges.iter().find_map(|&eidx| {
            let e = &facts.edges[eidx];
            match e.callee {
                CalleeRef::Project(id) if id == fn_id => Some(e),
                _ => None,
            }
        });
        // If the sink is inside the entry itself, there is no caller arg — unknown.
        let Some(edge) = call_edge else {
            any_unknown = true;
            continue;
        };
        let lit = edge
            .positional_literals
            .get(guard.index as usize)
            .copied()
            .flatten();
        match (lit, guard.inverted) {
            (Some(LiteralArg::True), false) | (Some(LiteralArg::False), true) => {}
            (Some(LiteralArg::False), false)
            | (Some(LiteralArg::None), false)
            | (Some(LiteralArg::True), true) => {
                return ParamVerdict::ProvablyFalse;
            }
            _ => any_unknown = true,
        }
    }
    if any_unknown {
        ParamVerdict::Unknown
    } else {
        ParamVerdict::ProvablyTrue
    }
}

/// Merge `open(...)` + subsequent `f.read()`/`f.write()` findings for the same entry.
fn merge_file_io_findings(findings: Vec<Finding>) -> Vec<Finding> {
    let mut opens: Vec<Finding> = Vec::new();
    let mut others: Vec<Finding> = Vec::new();
    for f in findings {
        let sink_text = f.chain.last().map(|c| c.text.as_str()).unwrap_or("");
        if is_open_call_text(sink_text)
            || f.fix.fix_group_id.contains("open") && f.rule == "blocking-file"
        {
            // Prefer matching by last chain hop looking like open(
            if is_open_call_text(sink_text) {
                opens.push(f);
                continue;
            }
        }
        if f.rule == "blocking-file" && is_file_rw_text(sink_text) {
            others.push(f);
            continue;
        }
        others.push(f);
    }

    // For each open finding, absorb same-entry file rw findings in the same file.
    let mut absorbed: HashSet<usize> = HashSet::new();
    for open in &mut opens {
        for (i, rw) in others.iter().enumerate() {
            if absorbed.contains(&i) {
                continue;
            }
            if rw.rule != "blocking-file"
                || !is_file_rw_text(rw.chain.last().map(|c| c.text.as_str()).unwrap_or(""))
            {
                continue;
            }
            if rw.entry.file != open.entry.file || rw.entry.name != open.entry.name {
                continue;
            }
            // Same entry: append rw hop if not already present.
            for step in &rw.chain {
                if !open
                    .chain
                    .iter()
                    .any(|s| s.file == step.file && s.line == step.line && s.text == step.text)
                {
                    open.chain.push(step.clone());
                }
            }
            absorbed.insert(i);
        }
    }

    let mut out = opens;
    for (i, f) in others.into_iter().enumerate() {
        if !absorbed.contains(&i) {
            out.push(f);
        }
    }
    out
}

fn is_open_call_text(text: &str) -> bool {
    let t = text.trim();
    t.starts_with("open(") || t.contains(".open(")
}

fn is_file_rw_text(text: &str) -> bool {
    let t = text.trim();
    t.contains(".read(")
        || t.contains(".readline(")
        || t.contains(".readlines(")
        || t.contains(".write(")
        || t.contains(".writelines(")
}

struct PathToEntry {
    entry: FunctionId,
    /// Edges from entry toward the seed caller (may be empty if seed is inside entry).
    edges: Vec<usize>,
}

fn shortest_paths_to_async_entries(
    facts: &ProjectFacts,
    reverse: &HashMap<FunctionId, Vec<usize>>,
    seed_caller: FunctionId,
) -> Vec<PathToEntry> {
    let seed_fn = facts.function(seed_caller);
    if seed_fn.is_async && !seed_fn.is_sync_route {
        return vec![PathToEntry {
            entry: seed_caller,
            edges: vec![],
        }];
    }

    let mut via_edge: HashMap<FunctionId, usize> = HashMap::new();
    let mut visited: HashSet<FunctionId> = HashSet::new();
    let mut queue: VecDeque<FunctionId> = VecDeque::new();
    queue.push_back(seed_caller);
    visited.insert(seed_caller);

    let mut entries = Vec::new();

    while let Some(node) = queue.pop_front() {
        let Some(preds) = reverse.get(&node) else {
            continue;
        };
        for &edge_idx in preds {
            let edge = &facts.edges[edge_idx];
            let pred = edge.caller;
            if !visited.insert(pred) {
                continue;
            }
            via_edge.insert(pred, edge_idx);
            let pred_fn = facts.function(pred);
            if pred_fn.is_async && !pred_fn.is_sync_route {
                let mut edges = Vec::new();
                let mut cur = pred;
                while cur != seed_caller {
                    let eidx = via_edge[&cur];
                    edges.push(eidx);
                    match facts.edges[eidx].callee {
                        CalleeRef::Project(next) => cur = next,
                        _ => break,
                    }
                }
                entries.push(PathToEntry { entry: pred, edges });
            } else {
                queue.push_back(pred);
            }
        }
    }

    entries
}

fn async_twin_hint(
    facts: &ProjectFacts,
    entry_id: FunctionId,
    path: &PathToEntry,
) -> Option<String> {
    let host = if let Some(&last) = path.edges.last() {
        match facts.edges[last].callee {
            CalleeRef::Project(fid) => facts.function(fid),
            _ => facts.function(entry_id),
        }
    } else {
        facts.function(entry_id)
    };

    let method = host.qualname.rsplit('.').next().unwrap_or(&host.qualname);
    for (sync_name, twin) in &host.async_twins {
        if sync_name == method {
            let class = host.qualname.rsplit_once('.').map(|(c, _)| c).unwrap_or("");
            let q = if class.is_empty() {
                twin.clone()
            } else {
                format!("{class}.{twin}")
            };
            return Some(q);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{CallEdge, CallKind, FunctionFact, LiteralArg, ParamGuard, ProjectFacts};
    use crate::report::EntryLabel;

    struct TestSinks;
    impl SinkTable for TestSinks {
        fn lookup(&self, qualname: &str) -> Option<SinkHit> {
            match qualname {
                "time.sleep" => Some(SinkHit {
                    rule: "blocking-sleep",
                    sink_qualname: "time.sleep",
                    fix_hint: "use asyncio.sleep",
                    candidate_only: false,
                    candidate_reason: None,
                }),
                "builtins.open" => Some(SinkHit {
                    rule: "blocking-file",
                    sink_qualname: "builtins.open",
                    fix_hint: "offload",
                    candidate_only: false,
                    candidate_reason: None,
                }),
                "io.TextIOWrapper.read" => Some(SinkHit {
                    rule: "blocking-file",
                    sink_qualname: "io.TextIOWrapper.read",
                    fix_hint: "offload",
                    candidate_only: false,
                    candidate_reason: None,
                }),
                "os.stat" => Some(SinkHit {
                    rule: "blocking-file",
                    sink_qualname: "os.stat",
                    fix_hint: "offload",
                    candidate_only: true,
                    candidate_reason: Some("metadata"),
                }),
                _ => None,
            }
        }
    }

    fn fn_fact(id: u32, name: &str, is_async: bool, file: &str, line: u32) -> FunctionFact {
        FunctionFact {
            id: FunctionId(id),
            file: file.into(),
            line,
            module: "m".into(),
            qualname: name.into(),
            is_async,
            is_generator: false,
            label: EntryLabel::Other,
            is_sync_route: false,
            async_twins: vec![],
            param_names: vec![],
        }
    }

    fn edge(
        caller: u32,
        callee: CalleeRef,
        line: u32,
        text: &str,
        conditions: Vec<ConditionTag>,
    ) -> CallEdge {
        CallEdge {
            caller: FunctionId(caller),
            callee,
            file: "a.py".into(),
            line,
            text: text.into(),
            kind: CallKind::Call,
            conditions,
            occurrence: 0,
            is_offload_wrapper: false,
            positional_literals: vec![],
            param_guard: None,
        }
    }

    #[test]
    fn reconcile_strips_unconditional_when_branched() {
        let tags = [
            ConditionTag::Unconditional,
            ConditionTag::AllPaths,
            ConditionTag::AsyncBranch,
        ];
        let out = reconcile_conditions(&tags);
        assert!(out.contains(&"async-branch".to_string()));
        assert!(!out.iter().any(|s| s == "unconditional"));
        assert!(!out.iter().any(|s| s == "all-paths"));
    }

    #[test]
    fn param_guard_unknown_becomes_candidate() {
        let mut helper = fn_fact(1, "helper", false, "a.py", 5);
        helper.param_names = vec!["flag".into()];
        let facts = ProjectFacts {
            functions: vec![fn_fact(0, "handler", true, "a.py", 1), helper],
            edges: vec![
                {
                    let mut e = edge(
                        0,
                        CalleeRef::Project(FunctionId(1)),
                        2,
                        "helper(flag)",
                        vec![ConditionTag::Unconditional, ConditionTag::AllPaths],
                    );
                    e.positional_literals = vec![None]; // unknown
                    e
                },
                {
                    let mut e = edge(
                        1,
                        CalleeRef::External {
                            qualname: "time.sleep".into(),
                        },
                        6,
                        "time.sleep(1)",
                        vec![ConditionTag::AsyncBranch],
                    );
                    e.param_guard = Some(ParamGuard {
                        index: 0,
                        inverted: false,
                    });
                    e
                },
            ],
            complete: true,
            skipped: vec![],
        };
        let report = analyze(&facts, &TestSinks);
        assert!(report.findings.is_empty(), "{:?}", report.findings);
        assert_eq!(report.candidates.len(), 1);
        assert_eq!(report.candidates[0].reason, "conditional");
    }

    #[test]
    fn param_guard_false_literal_skipped() {
        let mut helper = fn_fact(1, "helper", false, "a.py", 5);
        helper.param_names = vec!["flag".into()];
        let facts = ProjectFacts {
            functions: vec![fn_fact(0, "handler", true, "a.py", 1), helper],
            edges: vec![
                {
                    let mut e = edge(
                        0,
                        CalleeRef::Project(FunctionId(1)),
                        2,
                        "helper(False)",
                        vec![ConditionTag::Unconditional],
                    );
                    e.positional_literals = vec![Some(LiteralArg::False)];
                    e
                },
                {
                    let mut e = edge(
                        1,
                        CalleeRef::External {
                            qualname: "time.sleep".into(),
                        },
                        6,
                        "time.sleep(1)",
                        vec![ConditionTag::AsyncBranch],
                    );
                    e.param_guard = Some(ParamGuard {
                        index: 0,
                        inverted: false,
                    });
                    e
                },
            ],
            complete: true,
            skipped: vec![],
        };
        let report = analyze(&facts, &TestSinks);
        assert!(report.findings.is_empty());
        assert!(report.candidates.is_empty());
    }

    #[test]
    fn skips_test_entries_by_default() {
        let facts = ProjectFacts {
            functions: vec![fn_fact(0, "test_handler", true, "tests/test_a.py", 1)],
            edges: vec![edge(
                0,
                CalleeRef::External {
                    qualname: "time.sleep".into(),
                },
                2,
                "time.sleep(1)",
                vec![ConditionTag::Unconditional, ConditionTag::AllPaths],
            )],
            complete: true,
            skipped: vec![],
        };
        let report = analyze(&facts, &TestSinks);
        assert!(report.findings.is_empty());
        assert_eq!(report.omitted_non_production_entries, 1);

        let report2 = analyze_with_options(
            &facts,
            &TestSinks,
            AnalyzeOptions {
                include_non_production: true,
                show_unresolved: false,
            },
        );
        assert_eq!(report2.findings.len(), 1);
    }

    #[test]
    fn hides_unresolved_by_default() {
        let facts = ProjectFacts {
            functions: vec![fn_fact(0, "handler", true, "a.py", 1)],
            edges: vec![edge(
                0,
                CalleeRef::Unresolved {
                    reason: "unresolved".into(),
                },
                2,
                "getattr(m, n)()",
                vec![ConditionTag::Unconditional],
            )],
            complete: true,
            skipped: vec![],
        };
        let report = analyze(&facts, &TestSinks);
        assert!(report.candidates.is_empty());
        assert_eq!(report.hidden_unresolved_candidates, 1);

        let report2 = analyze_with_options(
            &facts,
            &TestSinks,
            AnalyzeOptions {
                include_non_production: false,
                show_unresolved: true,
            },
        );
        assert_eq!(report2.candidates.len(), 1);
    }

    #[test]
    fn merges_open_and_read() {
        let facts = ProjectFacts {
            functions: vec![fn_fact(0, "handler", true, "a.py", 1)],
            edges: vec![
                edge(
                    0,
                    CalleeRef::External {
                        qualname: "builtins.open".into(),
                    },
                    2,
                    "open('x')",
                    vec![ConditionTag::Unconditional, ConditionTag::AllPaths],
                ),
                edge(
                    0,
                    CalleeRef::External {
                        qualname: "io.TextIOWrapper.read".into(),
                    },
                    3,
                    "f.read()",
                    vec![ConditionTag::Unconditional, ConditionTag::AllPaths],
                ),
            ],
            complete: true,
            skipped: vec![],
        };
        let report = analyze(&facts, &TestSinks);
        assert_eq!(report.findings.len(), 1, "{:?}", report.findings);
        assert!(
            report.findings[0]
                .chain
                .iter()
                .any(|c| c.text.contains("open")),
            "{:?}",
            report.findings[0].chain
        );
        assert!(
            report.findings[0]
                .chain
                .iter()
                .any(|c| c.text.contains("read")),
            "{:?}",
            report.findings[0].chain
        );
    }

    #[test]
    fn is_non_production_paths() {
        assert!(is_non_production_entry_path("tests/test_a.py"));
        assert!(is_non_production_entry_path("app/test_foo.py"));
        assert!(is_non_production_entry_path("app/foo_test.py"));
        assert!(is_non_production_entry_path("docs/guide.py"));
        assert!(is_non_production_entry_path("examples/demo.py"));
        assert!(is_non_production_entry_path(".agents/x.py"));
        assert!(!is_non_production_entry_path("app/main.py"));
    }
}
