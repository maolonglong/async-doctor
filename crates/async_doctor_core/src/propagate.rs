//! Blocking propagation: mark sinks, BFS reverse call graph, grade findings.

use std::collections::{HashMap, HashSet, VecDeque};

use crate::graph::{CalleeRef, ConditionTag, FunctionId, ProjectFacts};
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

/// Analyze project facts against a sink table and produce a report.
pub fn analyze(facts: &ProjectFacts, sinks: &dyn SinkTable) -> Report {
    let mut report = Report {
        schema_version: SCHEMA_VERSION,
        complete: facts.complete,
        findings: Vec::new(),
        candidates: Vec::new(),
        skipped: facts.skipped.clone(),
    };

    // Reverse adjacency: callee FunctionId → list of edge indices that call it.
    let mut reverse: HashMap<FunctionId, Vec<usize>> = HashMap::new();
    for (idx, edge) in facts.edges.iter().enumerate() {
        if edge.is_offload_wrapper {
            continue;
        }
        if let CalleeRef::Project(callee) = edge.callee {
            reverse.entry(callee).or_default().push(idx);
        }
    }

    // Seed: external sink edges + unresolved edges (candidates).
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

    // For each seed, BFS reverse to async entries; keep shortest chain.
    // Keyed by (entry, sink_qualname_or_unresolved, chain_qualnames, occurrence).
    let mut emitted: HashSet<String> = HashSet::new();

    for seed in seeds {
        let seed_edge = &facts.edges[seed.edge_idx];
        let seed_caller = seed_edge.caller;

        // Path from seed caller down to the sink edge (single hop at seed).
        // BFS builds path: entry --e0--> ... --ek--> seed_caller --seed--> sink
        let paths = shortest_paths_to_async_entries(facts, &reverse, seed_caller);

        for path in paths {
            let entry_id = path.entry;
            let entry = facts.function(entry_id);
            if entry.is_sync_route || !entry.is_async {
                continue;
            }

            let mut chain_steps: Vec<ChainStep> = Vec::new();
            let mut chain_qualnames: Vec<String> = Vec::new();
            let mut conditions: Vec<String> = Vec::new();

            // Hops from entry toward the seed caller.
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
                merge_conditions(&mut conditions, &e.conditions);
            }

            // Final hop: the seed edge itself.
            chain_steps.push(ChainStep {
                file: seed_edge.file.clone(),
                line: seed_edge.line,
                text: seed_edge.text.clone(),
            });
            merge_conditions(&mut conditions, &seed_edge.conditions);

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
                    if let Some(twin) = async_twin_hint(facts, entry_id, &path, hit.sink_qualname) {
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

                    if hit.candidate_only {
                        report.candidates.push(Candidate {
                            id,
                            rule: hit.rule.to_string(),
                            entry: entry_out,
                            chain: chain_steps,
                            conditions,
                            reason: hit.candidate_reason.unwrap_or("uncertain").to_string(),
                            fix,
                            why,
                        });
                    } else {
                        report.findings.push(Finding {
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
                    report.candidates.push(Candidate {
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

fn merge_conditions(out: &mut Vec<String>, tags: &[ConditionTag]) {
    for tag in tags {
        let s = tag.as_str().to_string();
        if !out.iter().any(|x| x == &s) {
            out.push(s);
        }
    }
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

    // BFS upward via reverse edges. `via_edge[caller]` is the edge caller→child
    // on the shortest path toward the seed.
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
                // Shortest path only: do not expand past async entries.
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
    _sink_qualname: &str,
) -> Option<String> {
    // Prefer twins recorded on the last project function before the sink
    // (e.g. `Client.fetch` → `afetch` when both exist on the same class).
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
            return Some(
                format!(
                    "{}.{}",
                    host.qualname.rsplit_once('.').map(|(c, _)| c).unwrap_or(""),
                    twin
                )
                .trim_start_matches('.')
                .to_string(),
            );
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{CallEdge, CallKind, FunctionFact, ProjectFacts};
    use crate::report::EntryLabel;

    struct TestSinks;
    impl SinkTable for TestSinks {
        fn lookup(&self, qualname: &str) -> Option<SinkHit> {
            if qualname == "time.sleep" {
                Some(SinkHit {
                    rule: "blocking-sleep",
                    sink_qualname: "time.sleep",
                    fix_hint: "use asyncio.sleep",
                    candidate_only: false,
                    candidate_reason: None,
                })
            } else if qualname == "os.stat" {
                Some(SinkHit {
                    rule: "blocking-file",
                    sink_qualname: "os.stat",
                    fix_hint: "offload",
                    candidate_only: true,
                    candidate_reason: Some("metadata"),
                })
            } else {
                None
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
            label: EntryLabel::Other,
            is_sync_route: false,
            async_twins: vec![],
        }
    }

    #[test]
    fn direct_sink_in_async_def() {
        let facts = ProjectFacts {
            functions: vec![fn_fact(0, "handler", true, "a.py", 1)],
            edges: vec![CallEdge {
                caller: FunctionId(0),
                callee: CalleeRef::External {
                    qualname: "time.sleep".into(),
                },
                file: "a.py".into(),
                line: 2,
                text: "time.sleep(1)".into(),
                kind: CallKind::Call,
                conditions: vec![ConditionTag::Unconditional],
                occurrence: 0,
                is_offload_wrapper: false,
            }],
            complete: true,
            skipped: vec![],
        };
        let report = analyze(&facts, &TestSinks);
        assert_eq!(report.findings.len(), 1);
        assert_eq!(report.findings[0].rule, "blocking-sleep");
        assert_eq!(report.findings[0].chain.len(), 1);
    }

    #[test]
    fn multi_hop_shortest_chain() {
        let facts = ProjectFacts {
            functions: vec![
                fn_fact(0, "handler", true, "a.py", 1),
                fn_fact(1, "helper", false, "b.py", 1),
            ],
            edges: vec![
                CallEdge {
                    caller: FunctionId(0),
                    callee: CalleeRef::Project(FunctionId(1)),
                    file: "a.py".into(),
                    line: 2,
                    text: "helper()".into(),
                    kind: CallKind::Call,
                    conditions: vec![ConditionTag::Unconditional],
                    occurrence: 0,
                    is_offload_wrapper: false,
                },
                CallEdge {
                    caller: FunctionId(1),
                    callee: CalleeRef::External {
                        qualname: "time.sleep".into(),
                    },
                    file: "b.py".into(),
                    line: 2,
                    text: "time.sleep(1)".into(),
                    kind: CallKind::Call,
                    conditions: vec![ConditionTag::Unconditional],
                    occurrence: 0,
                    is_offload_wrapper: false,
                },
            ],
            complete: true,
            skipped: vec![],
        };
        let report = analyze(&facts, &TestSinks);
        assert_eq!(report.findings.len(), 1);
        assert_eq!(report.findings[0].chain.len(), 2);
        assert_eq!(report.findings[0].chain[0].text, "helper()");
        assert_eq!(report.findings[0].chain[1].text, "time.sleep(1)");
    }

    #[test]
    fn offload_wrapper_cuts_propagation() {
        // helper is only reached via an offload wrapper edge (not recorded as Project edge).
        let facts = ProjectFacts {
            functions: vec![
                fn_fact(0, "handler", true, "a.py", 1),
                fn_fact(1, "helper", false, "b.py", 1),
            ],
            edges: vec![
                CallEdge {
                    caller: FunctionId(0),
                    callee: CalleeRef::Offload(crate::graph::OffloadKind::AsyncioToThread),
                    file: "a.py".into(),
                    line: 2,
                    text: "asyncio.to_thread(helper)".into(),
                    kind: CallKind::Call,
                    conditions: vec![ConditionTag::Unconditional],
                    occurrence: 0,
                    is_offload_wrapper: true,
                },
                CallEdge {
                    caller: FunctionId(1),
                    callee: CalleeRef::External {
                        qualname: "time.sleep".into(),
                    },
                    file: "b.py".into(),
                    line: 2,
                    text: "time.sleep(1)".into(),
                    kind: CallKind::Call,
                    conditions: vec![ConditionTag::Unconditional],
                    occurrence: 0,
                    is_offload_wrapper: false,
                },
            ],
            complete: true,
            skipped: vec![],
        };
        let report = analyze(&facts, &TestSinks);
        assert!(report.findings.is_empty());
    }

    #[test]
    fn cycle_does_not_loop_forever() {
        let facts = ProjectFacts {
            functions: vec![
                fn_fact(0, "handler", true, "a.py", 1),
                fn_fact(1, "a", false, "a.py", 5),
                fn_fact(2, "b", false, "a.py", 9),
            ],
            edges: vec![
                CallEdge {
                    caller: FunctionId(0),
                    callee: CalleeRef::Project(FunctionId(1)),
                    file: "a.py".into(),
                    line: 2,
                    text: "a()".into(),
                    kind: CallKind::Call,
                    conditions: vec![ConditionTag::Unconditional],
                    occurrence: 0,
                    is_offload_wrapper: false,
                },
                CallEdge {
                    caller: FunctionId(1),
                    callee: CalleeRef::Project(FunctionId(2)),
                    file: "a.py".into(),
                    line: 6,
                    text: "b()".into(),
                    kind: CallKind::Call,
                    conditions: vec![ConditionTag::Unconditional],
                    occurrence: 0,
                    is_offload_wrapper: false,
                },
                CallEdge {
                    caller: FunctionId(2),
                    callee: CalleeRef::Project(FunctionId(1)),
                    file: "a.py".into(),
                    line: 10,
                    text: "a()".into(),
                    kind: CallKind::Call,
                    conditions: vec![ConditionTag::Unconditional],
                    occurrence: 0,
                    is_offload_wrapper: false,
                },
                CallEdge {
                    caller: FunctionId(2),
                    callee: CalleeRef::External {
                        qualname: "time.sleep".into(),
                    },
                    file: "a.py".into(),
                    line: 11,
                    text: "time.sleep(1)".into(),
                    kind: CallKind::Call,
                    conditions: vec![ConditionTag::Unconditional],
                    occurrence: 0,
                    is_offload_wrapper: false,
                },
            ],
            complete: true,
            skipped: vec![],
        };
        let report = analyze(&facts, &TestSinks);
        assert_eq!(report.findings.len(), 1);
    }

    #[test]
    fn unresolved_becomes_candidate() {
        let facts = ProjectFacts {
            functions: vec![fn_fact(0, "handler", true, "a.py", 1)],
            edges: vec![CallEdge {
                caller: FunctionId(0),
                callee: CalleeRef::Unresolved {
                    reason: "unresolved".into(),
                },
                file: "a.py".into(),
                line: 2,
                text: "getattr(m, n)()".into(),
                kind: CallKind::Call,
                conditions: vec![ConditionTag::Unconditional],
                occurrence: 0,
                is_offload_wrapper: false,
            }],
            complete: true,
            skipped: vec![],
        };
        let report = analyze(&facts, &TestSinks);
        assert!(report.findings.is_empty());
        assert_eq!(report.candidates.len(), 1);
        assert_eq!(report.candidates[0].reason, "unresolved");
    }
}
