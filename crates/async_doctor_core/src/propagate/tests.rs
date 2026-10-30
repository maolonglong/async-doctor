//! Unit tests for propagation grading.

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
                with_only: false,
            }),
            "builtins.open" => Some(SinkHit {
                rule: "blocking-file",
                sink_qualname: "builtins.open",
                fix_hint: "offload",
                candidate_only: false,
                candidate_reason: None,
                with_only: false,
            }),
            "io.TextIOWrapper.read" => Some(SinkHit {
                rule: "blocking-file",
                sink_qualname: "io.TextIOWrapper.read",
                fix_hint: "offload",
                candidate_only: false,
                candidate_reason: None,
                with_only: false,
            }),
            "os.stat" => Some(SinkHit {
                rule: "blocking-file",
                sink_qualname: "os.stat",
                fix_hint: "offload",
                candidate_only: true,
                candidate_reason: Some("metadata"),
                with_only: false,
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
        param_guard: None,
        is_awaited: false,
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
        skipped: vec![],
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
