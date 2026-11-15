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

async def starred_tuple_literal_not_consumed():
    wrap_starred(*(sync_chunks(),))

async def starred_list_literal_not_consumed():
    wrap_starred(*[sync_chunks()])

async def starred_set_literal_not_consumed():
    wrap_starred(*{sync_chunks()})

async def starred_nested_starred_consumes():
    wrap_starred(*[*sync_chunks()])

def kw_wrap(**kwargs):
    return kwargs

async def doublestar_dict_literal_not_consumed():
    kw_wrap(**{"g": sync_chunks()})

async def doublestar_mapping_not_consumed():
    m = {"g": sync_chunks()}
    kw_wrap(**m)

async def product_repeat_zero_not_consumed():
    for item in itertools.product(sync_chunks(), repeat=0):
        pass

async def product_repeat_zero_bare():
    itertools.product(sync_chunks(), repeat=0)

async def product_repeat_two_consumes():
    for item in itertools.product(sync_chunks(), repeat=2):
        pass

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
        "starred_tuple_literal_not_consumed",
        "starred_list_literal_not_consumed",
        "starred_set_literal_not_consumed",
        "doublestar_dict_literal_not_consumed",
        "doublestar_mapping_not_consumed",
        "product_repeat_zero_not_consumed",
        "product_repeat_zero_bare",
    ] {
        assert!(
            !names.contains(clean),
            "{clean} passes the generator to a callable; the pass itself must not consume it on the loop: {:?}",
            report.findings
        );
    }
    for blocked in [
        "product_arg_consumes",
        "product_repeat_two_consumes",
        "starred_nested_starred_consumes",
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

#[test]
fn body_only_proofs_do_not_escape_loops_try_or_aliases() {
    let dir = super::helpers::temp_project(&[(
        "leak.py",
        r#"
import os
import socket
import sqlite3

async def fd_nonblocking_only_in_loop_blocks(items: list):
    r, w = os.pipe()
    for _ in items:
        os.set_blocking(r, False)
    os.read(r, 1)

async def fd_nonblocking_only_in_try_blocks():
    r, w = os.pipe()
    try:
        os.set_blocking(r, False)
    except Exception:
        pass
    os.read(r, 1)

async def fd_nonblocking_only_in_else_blocks(items: list):
    r, w = os.pipe()
    for _ in items:
        pass
    else:
        os.set_blocking(r, False)
    os.read(r, 1)

async def fd_alias_drops_proof_blocks():
    r, w = os.pipe2(os.O_NONBLOCK)
    alias = r
    os.set_blocking(alias, True)
    os.read(r, 1)

async def sock_nonblocking_only_in_loop_blocks(items: list):
    sock = socket.socket()
    for _ in items:
        sock.setblocking(False)
    sock.recv(1)

async def sqlite_mem_only_in_loop_blocks(items: list):
    conn = sqlite3.connect("app.db")
    for _ in items:
        conn = sqlite3.connect(":memory:")
    conn.execute("SELECT 1")

async def fd_nonblocking_before_loop_stays_clean(items: list):
    r, w = os.pipe()
    os.set_blocking(r, False)
    for _ in items:
        os.read(r, 1024)

async def fd_reblocking_in_try_body_except_blocks():
    r, w = os.pipe2(os.O_NONBLOCK)
    try:
        os.set_blocking(r, True)
        x()
    except Exception:
        os.read(r, 1)

async def fd_reblocking_in_try_body_finally_blocks():
    r, w = os.pipe2(os.O_NONBLOCK)
    try:
        os.set_blocking(r, True)
        x()
    except Exception:
        pass
    finally:
        os.read(r, 1)

async def sock_reblocking_in_try_body_blocks():
    sock = socket.socket()
    sock.setblocking(False)
    try:
        sock.settimeout(1.0)
        x()
    except Exception:
        sock.recv(1)

async def fd_nonblocking_survives_clean_try_ok():
    r, w = os.pipe2(os.O_NONBLOCK)
    try:
        x()
    except Exception:
        os.read(r, 1)

async def fd_setblocking_read_in_loop_ok(items: list):
    r, w = os.pipe()
    for _ in items:
        os.set_blocking(r, False)
        os.read(r, 1)

async def fd_mode_call_in_nested_def_ok():
    r, w = os.pipe2(os.O_NONBLOCK)
    try:
        def helper():
            os.set_blocking(r, True)
        x()
    except Exception:
        os.read(r, 1)

async def fd_other_obj_set_blocking_ok():
    r, w = os.pipe2(os.O_NONBLOCK)
    try:
        other.set_blocking(r, True)
        x()
    except Exception:
        os.read(r, 1)
"#,
    )]);
    let report = scan_path(&dir).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    let names: std::collections::BTreeSet<_> = report
        .findings
        .iter()
        .map(|f| f.entry.name.as_str())
        .collect();
    for clean in [
        "fd_setblocking_read_in_loop_ok",
        "fd_mode_call_in_nested_def_ok",
        "fd_other_obj_set_blocking_ok",
    ] {
        assert!(
            !names.contains(clean),
            "{clean}: the proof must not be undone: {names:?}"
        );
    }
    assert!(
        !names.contains("fd_nonblocking_survives_clean_try_ok"),
        "a proof valid before the try must hold inside the handler: {:?}",
        report.findings
    );
    assert!(
        !names.contains("fd_nonblocking_before_loop_stays_clean"),
        "proof established before the loop must apply inside it: {:?}",
        report.findings
    );
    for blocked in [
        "fd_nonblocking_only_in_loop_blocks",
        "fd_nonblocking_only_in_try_blocks",
        "fd_nonblocking_only_in_else_blocks",
        "fd_alias_drops_proof_blocks",
        "fd_reblocking_in_try_body_except_blocks",
        "fd_reblocking_in_try_body_finally_blocks",
        "sock_reblocking_in_try_body_blocks",
        "sock_nonblocking_only_in_loop_blocks",
        "sqlite_mem_only_in_loop_blocks",
    ] {
        assert!(
            names.contains(blocked),
            "{blocked}: a body-only or aliased-away proof must not survive: {names:?}"
        );
    }
}
