//! Unit tests for propagation grading.

use std::collections::HashSet;

use super::*;
use crate::graph::{CallEdge, CallKind, FunctionFact, LiteralArg, ParamGuard, ProjectFacts};
use crate::report::EntryLabel;

struct TestSinks;
impl SinkTable for TestSinks {
    fn lookup(&self, qualname: &str) -> Option<SinkHit> {
        match qualname {
            // Family-style alias: two callees share one sink key (boto Session.client shape).
            "pkg.factory_a" | "pkg.factory_b" => Some(SinkHit {
                rule: "blocking-http",
                sink_qualname: "pkg.factory",
                fix_hint: "use async twin",
                candidate_only: false,
                candidate_reason: None,
                with_only: false,
                consume_only: false,
            }),
            "time.sleep" => Some(SinkHit {
                rule: "blocking-sleep",
                sink_qualname: "time.sleep",
                fix_hint: "use asyncio.sleep",
                candidate_only: false,
                candidate_reason: None,
                with_only: false,
                consume_only: false,
            }),
            "builtins.open" => Some(SinkHit {
                rule: "blocking-file",
                sink_qualname: "builtins.open",
                fix_hint: "offload",
                candidate_only: false,
                candidate_reason: None,
                with_only: false,
                consume_only: false,
            }),
            "io.TextIOWrapper.read" => Some(SinkHit {
                rule: "blocking-file",
                sink_qualname: "io.TextIOWrapper.read",
                fix_hint: "offload",
                candidate_only: false,
                candidate_reason: None,
                with_only: false,
                consume_only: false,
            }),
            "io.TextIOWrapper.write" => Some(SinkHit {
                rule: "blocking-file",
                sink_qualname: "io.TextIOWrapper.write",
                fix_hint: "offload",
                candidate_only: false,
                candidate_reason: None,
                with_only: false,
                consume_only: false,
            }),
            "os.stat" => Some(SinkHit {
                rule: "blocking-file",
                sink_qualname: "os.stat",
                fix_hint: "offload",
                candidate_only: true,
                candidate_reason: Some("metadata"),
                with_only: false,
                consume_only: false,
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
        is_third_party: false,
        package_name: None,
        package_version: None,
        invokes_params: vec![],
        is_offload_wrapper: false,
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
        positional_callables: vec![],
        positional_externals: vec![],
        positional_constructed_receivers: vec![],
        keyword_callables: vec![],
        param_guard: None,
        is_awaited: false,
        callee_is_coroutine: None,
        is_eager_offload_arg: false,
        context_from_caller: None,
        constructed_receiver: None,
    }
}

#[test]
fn hop_text_truncates_to_first_line() {
    assert_eq!(
        super::grade::truncate_hop_text(
            "self.request(\n            \"GET\",\n            url,\n        )"
        ),
        "self.request("
    );
    assert_eq!(super::grade::truncate_hop_text("  \n  fn()  \n"), "fn()");
    assert_eq!(super::grade::truncate_hop_text("single"), "single");
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
        ..Default::default()
    };
    let report = analyze(&facts, &TestSinks);
    assert!(report.findings.is_empty(), "{:?}", report.findings);
    assert_eq!(report.candidates.len(), 1);
    assert_eq!(report.candidates[0].reason, "conditional");
}

#[test]
fn entry_local_param_guard_stays_finding() {
    // async def handler(flag): if flag: sleep() — entry params are external input.
    let mut entry = fn_fact(0, "handler", true, "a.py", 1);
    entry.param_names = vec!["flag".into()];
    let facts = ProjectFacts {
        functions: vec![entry],
        edges: vec![{
            let mut e = edge(
                0,
                CalleeRef::External {
                    qualname: "time.sleep".into(),
                },
                3,
                "time.sleep(1)",
                vec![ConditionTag::AsyncBranch],
            );
            e.param_guard = Some(ParamGuard {
                index: 0,
                inverted: false,
            });
            e
        }],
        complete: true,
        ..Default::default()
    };
    let report = analyze(&facts, &TestSinks);
    assert_eq!(report.findings.len(), 1, "{:?}", report.findings);
    assert!(report.candidates.is_empty(), "{:?}", report.candidates);
    assert!(
        report.findings[0]
            .conditions
            .iter()
            .any(|c| c == "async-branch")
    );
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
        ..Default::default()
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
        ..Default::default()
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
        ..Default::default()
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
        ..Default::default()
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
fn merges_multiple_writes_into_one_open() {
    // Snapshot prefixes before absorption so the second write is not orphaned.
    let facts = ProjectFacts {
        functions: vec![fn_fact(0, "handler", true, "a.py", 1)],
        edges: vec![
            edge(
                0,
                CalleeRef::External {
                    qualname: "builtins.open".into(),
                },
                2,
                "open(p, 'w')",
                vec![ConditionTag::Unconditional, ConditionTag::AllPaths],
            ),
            edge(
                0,
                CalleeRef::External {
                    qualname: "io.TextIOWrapper.write".into(),
                },
                3,
                "f.write('a')",
                vec![ConditionTag::Unconditional, ConditionTag::AllPaths],
            ),
            {
                let mut e = edge(
                    0,
                    CalleeRef::External {
                        qualname: "io.TextIOWrapper.write".into(),
                    },
                    4,
                    "f.write('b')",
                    vec![ConditionTag::Unconditional, ConditionTag::AllPaths],
                );
                e.occurrence = 1;
                e
            },
        ],
        complete: true,
        ..Default::default()
    };
    let report = analyze(&facts, &TestSinks);
    assert_eq!(
        report.findings.len(),
        1,
        "expected one merged finding, got {:?}",
        report.findings
    );
    let texts: Vec<_> = report.findings[0]
        .chain
        .iter()
        .map(|c| c.text.as_str())
        .collect();
    assert!(texts.iter().any(|t| t.contains("open")), "{texts:?}");
    assert_eq!(
        texts.iter().filter(|t| t.contains("write")).count(),
        2,
        "{texts:?}"
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

#[test]
fn aliased_sink_keys_need_distinct_occurrence() {
    // Two call sites whose callees alias to the same sink_qualname must not
    // share occurrence 0 — that collides finding ids and silently drops one.
    let facts = ProjectFacts {
        functions: vec![fn_fact(0, "handler", true, "a.py", 1)],
        edges: vec![
            {
                let mut e = edge(
                    0,
                    CalleeRef::External {
                        qualname: "pkg.factory_a".into(),
                    },
                    2,
                    "factory_a()",
                    vec![ConditionTag::Unconditional, ConditionTag::AllPaths],
                );
                e.occurrence = 0;
                e
            },
            {
                let mut e = edge(
                    0,
                    CalleeRef::External {
                        qualname: "pkg.factory_b".into(),
                    },
                    3,
                    "factory_b()",
                    vec![ConditionTag::Unconditional, ConditionTag::AllPaths],
                );
                // Distinct ordinal (as extract assigns when counting by sink key).
                e.occurrence = 1;
                e
            },
        ],
        complete: true,
        ..Default::default()
    };
    let report = analyze(&facts, &TestSinks);
    assert_eq!(
        report.findings.len(),
        2,
        "aliased factories with distinct occurrence must both emit: {:?}",
        report.findings
    );
    assert_ne!(report.findings[0].id, report.findings[1].id);
    let lines: HashSet<_> = report.findings.iter().map(|f| f.chain[0].line).collect();
    assert_eq!(lines, HashSet::from([2, 3]), "{:?}", report.findings);
}
