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

#[test]
fn generator_arg_to_async_for_callee_is_not_consumed_on_loop() {
    let dir = super::helpers::temp_project(&[(
        "app.py",
        r#"
import itertools
import time

def sync_chunks():
    with open("big.bin", "rb") as f:
        while chunk := f.read(65536):
            yield chunk

async def external_iter_arg_not_consumed():
    async for chunk in missing_iterate_in_threadpool(sync_chunks()):
        pass

async def project_iter_arg_not_consumed():
    for chunk in plain_wrapper(sync_chunks()):
        pass

def plain_wrapper(it):
    return it

async def adapter_enumerate_consumes():
    for i, c in enumerate(sync_chunks()):
        pass

async def adapter_zip_consumes():
    for a, b in zip(sync_chunks(), range(3)):
        pass

async def adapter_islice_consumes():
    for c in itertools.islice(sync_chunks(), 2):
        pass

async def adapter_nested_consumes():
    for i, (a, b) in enumerate(zip(sync_chunks(), range(3))):
        pass

async def adapter_comprehension_map_consumes():
    return [len(c) for c in map(bytes, sync_chunks())]

def wrap_starred(*args):
    return list(args)

async def product_arg_consumes():
    for item in itertools.product(sync_chunks()):
        pass

async def starred_arg_consumes_in_for():
    for item in wrap_starred(*sync_chunks()):
        pass

async def starred_arg_consumes_bare():
    wrap_starred(*sync_chunks())

async def sync_for_still_consumes():
    for chunk in sync_chunks():
        pass

async def list_still_consumes():
    return list(sync_chunks())
"#,
    )]);
    let report = scan_path(&dir).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    let names: BTreeSet<_> = report
        .findings
        .iter()
        .map(|f| f.entry.name.as_str())
        .collect();
    for clean in [
        "external_iter_arg_not_consumed",
        "project_iter_arg_not_consumed",
    ] {
        assert!(
            !names.contains(clean),
            "{clean} passes the generator to a callable; the pass itself must not consume it on the loop: {:?}",
            report.findings
        );
    }
    for blocked in [
        "product_arg_consumes",
        "starred_arg_consumes_in_for",
        "starred_arg_consumes_bare",
        "adapter_enumerate_consumes",
        "adapter_zip_consumes",
        "adapter_islice_consumes",
        "adapter_nested_consumes",
        "adapter_comprehension_map_consumes",
        "sync_for_still_consumes",
        "list_still_consumes",
    ] {
        assert!(
            names.contains(blocked),
            "{blocked} consumes the generator on the loop and must report: {names:?}"
        );
    }
}
