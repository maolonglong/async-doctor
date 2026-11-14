//! offload integration tests.

use std::fs;

use super::helpers::temp_project;
use crate::scan_path;

#[test]
fn offload_to_thread_cuts() {
    let dir = temp_project(&[(
        "off.py",
        "import asyncio\nimport time\n\ndef work():\n    time.sleep(1)\n\nasync def handler():\n    await asyncio.to_thread(work)\n",
    )]);
    let report = scan_path(&dir).unwrap();
    assert!(
        report.findings.is_empty(),
        "to_thread should cut: {:?}",
        report.findings
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn offload_lambda_cuts() {
    let dir = temp_project(&[(
        "lam.py",
        "import asyncio\nimport time\n\nasync def handler():\n    await asyncio.to_thread(lambda: time.sleep(1))\n",
    )]);
    let report = scan_path(&dir).unwrap();
    assert!(
        report.findings.is_empty(),
        "lambda offload should cut: {:?}",
        report.findings
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn offload_eager_call_still_reports() {
    let dir = temp_project(&[(
        "eager.py",
        "import asyncio\nimport time\n\ndef work():\n    time.sleep(1)\n    return 1\n\nasync def handler():\n    await asyncio.to_thread(work())\n",
    )]);
    let report = scan_path(&dir).unwrap();
    assert!(
        !report.findings.is_empty(),
        "eager work() must still report: {:?}",
        report.findings
    );
    assert!(
        report.findings.iter().any(|f| f.chain.iter().any(|step| {
            step.resolution
                .contains("argument evaluated on the event loop before offload")
        })),
        "eager evaluation must remain diagnostic evidence: {:?}",
        report.findings
    );
    assert!(report.findings.iter().all(|f| f.fix.hint.is_empty()));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn run_in_executor_cuts() {
    let dir = temp_project(&[(
        "exec.py",
        "import asyncio\nimport time\n\ndef work():\n    time.sleep(1)\n\nasync def handler():\n    loop = asyncio.get_running_loop()\n    await loop.run_in_executor(None, work)\n",
    )]);
    let report = scan_path(&dir).unwrap();
    assert!(
        report.findings.is_empty(),
        "run_in_executor should cut: {:?}",
        report.findings
    );
    let _ = fs::remove_dir_all(dir);
}

/// `sync_to_async` / `database_sync_to_async` / project offload decorators cut
/// the call edge — the decorated body never runs on the caller's loop.
#[test]
fn offload_decorators_cut() {
    use super::helpers::clone_fixture;
    let dir = clone_fixture("m4_offload_decorators");
    let report = scan_path(&dir).unwrap();
    let cut_entries = [
        "via_bare",
        "via_called",
        "via_db",
        "via_project",
        "via_project_indirect",
    ];
    for entry in cut_entries {
        assert!(
            !report.findings.iter().any(|f| f.entry.name == entry),
            "{entry} must have no findings (offload decorator cut): {:?}",
            report.findings
        );
    }
    // Transparent decorators must keep reporting.
    for entry in ["via_fake_name", "via_retry"] {
        assert!(
            report.findings.iter().any(|f| f.entry.name == entry),
            "{entry} must keep its blocking-sleep finding: {:?}",
            report.findings
        );
    }
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn offload_wrappers_with_partial_and_contextvars_preserve_cut_coro_and_twin() {
    let dir = temp_project(&[(
        "wrappers.py",
        r#"
import asyncio
import contextvars
import functools
import time
import anyio.to_thread
import httpx

def sync_sleep():
    time.sleep(1)

def sync_http():
    return httpx.get("https://example.com")

async def async_target():
    return 1

async def wrap_ctx_and_partial(func, *args, **kwargs):
    loop = asyncio.get_running_loop()
    ctx = contextvars.copy_context()
    call = functools.partial(func, *args, **kwargs)
    return await loop.run_in_executor(None, ctx.run, call)

async def wrap_partial_ctx_run(func, *args, **kwargs):
    loop = asyncio.get_running_loop()
    ctx = contextvars.copy_context()
    call = functools.partial(ctx.run, func, *args, **kwargs)
    return await loop.run_in_executor(None, call)

async def wrap_anyio_partial(func, *args, **kwargs):
    return await anyio.to_thread.run_sync(functools.partial(func, **kwargs), *args)

async def cut_via_ctx_partial_ok():
    await wrap_ctx_and_partial(sync_sleep)
    await wrap_partial_ctx_run(sync_sleep)
    await wrap_anyio_partial(sync_sleep)

async def coro_via_ctx_wrapper_bad():
    await wrap_ctx_and_partial(async_target)

async def coro_via_direct_ctx_run_bad():
    loop = asyncio.get_running_loop()
    await loop.run_in_executor(None, contextvars.copy_context().run, async_target)

async def twin_via_ctx_wrapper():
    await wrap_ctx_and_partial(sync_http)

async def twin_via_direct_ctx_run():
    loop = asyncio.get_running_loop()
    ctx = contextvars.copy_context()
    await loop.run_in_executor(None, ctx.run, httpx.get, "https://example.com")
"#,
    )]);
    super::helpers::copy_dir(&super::helpers::advice_shared_venv(), &dir.join(".venv"));
    let anyio_dir = dir.join(".venv/lib/python3.12/site-packages/anyio");
    fs::create_dir_all(&anyio_dir).unwrap();
    fs::write(anyio_dir.join("__init__.py"), "").unwrap();
    fs::write(
        anyio_dir.join("to_thread.py"),
        "async def run_sync(func, *args):\n    return func(*args)\n",
    )
    .unwrap();
    let report = scan_path(&dir).unwrap();
    let _ = fs::remove_dir_all(dir);

    assert!(
        !report
            .findings
            .iter()
            .any(|f| f.entry.name == "cut_via_ctx_partial_ok"),
        "partial + contextvars wrappers must be recognized as offload cuts: {:?}",
        report.findings
    );
    for bad in ["coro_via_ctx_wrapper_bad", "coro_via_direct_ctx_run_bad"] {
        assert!(
            report
                .findings
                .iter()
                .any(|f| f.entry.name == bad && f.rule == "offload-coroutine"),
            "{bad} must emit offload-coroutine: {:?}",
            report.findings
        );
    }
    for adv in ["twin_via_ctx_wrapper", "twin_via_direct_ctx_run"] {
        assert!(
            report
                .advice
                .iter()
                .any(|a| a.entry.name == adv && a.rule == "offload-has-async-twin"),
            "{adv} must emit offload-has-async-twin: {:?}",
            report.advice
        );
    }
}

#[test]
fn offload_lambda_returning_coroutine_emits_offload_coroutine() {
    let dir = temp_project(&[(
        "lam_coro.py",
        r#"
import asyncio
import functools
import time

async def async_fn() -> int:
    return 1

async def async_gen():
    yield 1

def sync_fn() -> int:
    time.sleep(1)
    return 2

async def inline_lambda_coro_bad():
    await asyncio.to_thread(lambda: async_fn())

async def local_lambda_coro_bad():
    thunk = lambda: async_fn()
    await asyncio.to_thread(thunk)

async def sync_lambda_ok():
    await asyncio.to_thread(lambda: sync_fn())

async def async_gen_lambda_not_coro():
    await asyncio.to_thread(lambda: async_gen())

async def async_gen_direct_not_coro():
    rows = await asyncio.to_thread(async_gen)
    return [row async for row in rows]

async def async_gen_partial_not_coro():
    rows = await asyncio.to_thread(functools.partial(async_gen))
    return [row async for row in rows]
"#,
    )]);
    let report = scan_path(&dir).unwrap();
    let _ = fs::remove_dir_all(dir);

    for bad in ["inline_lambda_coro_bad", "local_lambda_coro_bad"] {
        assert!(
            report
                .findings
                .iter()
                .any(|f| f.entry.name == bad && f.rule == "offload-coroutine"),
            "{bad} must emit offload-coroutine: {:?}",
            report.findings
        );
    }
    for good in [
        "sync_lambda_ok",
        "async_gen_lambda_not_coro",
        "async_gen_direct_not_coro",
        "async_gen_partial_not_coro",
    ] {
        assert!(
            !report.findings.iter().any(|f| f.entry.name == good),
            "{good} must not emit offload-coroutine: {:?}",
            report.findings
        );
    }
    assert_eq!(
        report.async_entry_count, 8,
        "synthetic lambda facts must not inflate async_entry_count"
    );
}
