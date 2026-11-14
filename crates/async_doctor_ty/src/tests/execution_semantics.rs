//! Public scan contracts for Python execution, not just callable syntax.

use std::collections::BTreeSet;

use super::helpers::fixtures_dir;
use crate::scan_path;

#[test]
fn coroutine_protocol_use_diagnoses_without_an_async_protocol_rewrite() {
    let report = scan_path(&fixtures_dir().join("python_execution")).unwrap();
    for name in [
        "coroutine_sync_for",
        "coroutine_async_for",
        "coroutine_named_for",
        "coroutine_sync_with",
        "coroutine_async_with",
    ] {
        let findings: Vec<_> = report
            .findings
            .iter()
            .filter(|f| f.entry.name == name)
            .collect();
        assert_eq!(findings.len(), 1, "{name}: {findings:?}");
        assert_eq!(findings[0].rule, "unawaited-coroutine", "{name}");
        assert!(
            findings[0].fix.hint.is_empty(),
            "{name}: {:?}",
            findings[0].fix
        );
    }
    assert!(
        !report
            .findings
            .iter()
            .any(|f| f.entry.name == "coroutine_awaited_ok"),
        "awaiting the returned list/context manager is valid: {:?}",
        report.findings
    );
    assert!(
        report.findings.iter().any(|f| {
            f.entry.name == "coroutine_nested_generator_lambda" && f.rule == "offload-coroutine"
        }),
        "yield in a nested lambda does not turn its enclosing coroutine into an async generator"
    );
}

#[test]
fn offload_wrappers_preserve_their_own_event_loop_work() {
    let report = scan_path(&fixtures_dir().join("python_execution")).unwrap();
    let names: BTreeSet<_> = report
        .findings
        .iter()
        .filter(|f| f.rule == "blocking-sleep" && f.entry.name.starts_with("wrapper_"))
        .map(|f| f.entry.name.as_str())
        .collect();
    assert_eq!(
        names,
        BTreeSet::from(["wrapper_preamble", "wrapper_postamble"]),
        "wrapper code runs on the loop; only its target runs on a worker"
    );
}

#[test]
fn named_generator_bodies_run_on_consumption_not_creation() {
    let report = scan_path(&fixtures_dir().join("python_execution")).unwrap();
    let names: BTreeSet<_> = report
        .findings
        .iter()
        .filter(|f| f.rule == "blocking-sleep" && f.entry.name.starts_with("generator_"))
        .map(|f| f.entry.name.as_str())
        .collect();
    assert_eq!(
        names,
        BTreeSet::from([
            "generator_alias",
            "generator_alias_after_rebind",
            "generator_created_offloop",
            "generator_for",
            "generator_forwarded_true",
            "generator_named",
            "generator_partial_worker_consumption",
        ]),
        "track deferred bodies and literal guards without treating creation as execution"
    );
}

#[test]
fn asyncio_callbacks_execute_on_the_event_loop_including_threadsafe_submission() {
    let report = scan_path(&fixtures_dir().join("python_execution")).unwrap();
    let names: BTreeSet<_> = report
        .findings
        .iter()
        .filter(|f| f.rule == "blocking-sleep" && f.entry.name.starts_with("callback_"))
        .map(|f| f.entry.name.as_str())
        .collect();
    assert_eq!(
        names,
        BTreeSet::from([
            "callback_argument_true",
            "callback_at",
            "callback_bound_method",
            "callback_external",
            "callback_future",
            "callback_future_partial",
            "callback_keyword",
            "callback_lambda_argument_true",
            "callback_later_partial",
            "callback_partial_argument_true",
            "callback_partial_keyword_true",
            "callback_rebound_parameter",
            "callback_soon",
            "callback_task",
            "callback_threadsafe",
        ]),
        "only typed asyncio scheduling invokes callbacks; forward callback arguments"
    );
}

#[test]
fn implicit_context_protocols_follow_local_and_inherited_method_bodies() {
    let report = scan_path(&fixtures_dir().join("python_execution")).unwrap();
    let names: BTreeSet<_> = report
        .findings
        .iter()
        .filter(|f| f.rule == "blocking-sleep" && f.entry.name.starts_with("context_"))
        .map(|f| f.entry.name.as_str())
        .collect();
    assert_eq!(
        names,
        BTreeSet::from([
            "context_async_enter",
            "context_enter_inherited",
            "context_enter_inline",
            "context_enter_named",
            "context_exit_inline",
            "context_exit_named",
        ]),
        "with invokes type-level __enter__/__exit__, unless the whole use is offloaded"
    );
}
