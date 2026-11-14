//! Regression tests (positives + strict FP guards) for LLM async advice rules:
//! - `offload-stateful-session`
//! - `useless-local-async-lock`
//! - `swallowed-cancelled-error`

use async_doctor_core::AdviceCategory;

use super::helpers::temp_project;
use crate::scan_path;

fn advice_pairs(report: &async_doctor_core::Report) -> Vec<(&str, &str)> {
    let mut pairs: Vec<(&str, &str)> = report
        .advice
        .iter()
        .map(|a| (a.rule.as_str(), a.entry.name.as_str()))
        .collect();
    pairs.sort_unstable();
    pairs.dedup();
    pairs
}

#[test]
fn offload_stateful_session_positives_and_negatives() {
    let dir = temp_project(&[
        (
            ".venv/pyvenv.cfg",
            "home = /usr/bin\nversion = 3.12.0\ninclude-system-site-packages = false\n",
        ),
        (
            ".venv/lib/python3.12/site-packages/sqlalchemy/__init__.py",
            "",
        ),
        (
            ".venv/lib/python3.12/site-packages/sqlalchemy/engine.py",
            "class Engine:\n    def connect(self):\n        return None\n",
        ),
        (
            ".venv/lib/python3.12/site-packages/sqlalchemy/orm/__init__.py",
            "from .session import Session as Session\n",
        ),
        (
            ".venv/lib/python3.12/site-packages/sqlalchemy/orm/session.py",
            "class Session:\n    def __init__(self, bind=None):\n        pass\n    def __enter__(self):\n        return self\n    def __exit__(self, *args):\n        return False\n    def execute(self, stmt):\n        return None\n    def commit(self):\n        return None\n",
        ),
        (
            ".venv/lib/python3.12/site-packages/sqlmodel/__init__.py",
            "class Session:\n    def __init__(self, bind=None):\n        pass\n    def exec(self, stmt):\n        return None\n",
        ),
        (
            "app.py",
            r#"import asyncio
import functools
import sqlite3
from sqlalchemy.engine import Engine
from sqlalchemy.orm import Session
from sqlmodel import Session as SqlModelSession


async def bad_sqlalchemy_to_thread(session: Session):
    return await asyncio.to_thread(session.execute, "SELECT 1")


async def bad_sqlmodel_partial(sm_session: SqlModelSession):
    return await asyncio.to_thread(functools.partial(sm_session.exec, "SELECT 1"))


async def bad_sqlite_conn_executor():
    loop = asyncio.get_running_loop()
    conn = sqlite3.connect(":memory:")
    return await loop.run_in_executor(None, conn.execute, "SELECT 1")


async def bad_sqlite_cursor_lambda():
    conn = sqlite3.connect(":memory:")
    cur = conn.cursor()
    return await asyncio.to_thread(lambda: cur.fetchall())


def _sync_unit_of_work(engine: Engine):
    with Session(engine) as s:
        return s.execute("SELECT 1")


async def safe_unit_of_work(engine: Engine):
    return await asyncio.to_thread(_sync_unit_of_work, engine)


async def safe_engine_connect(engine: Engine):
    return await asyncio.to_thread(engine.connect)


async def safe_lambda_opens_own():
    return await asyncio.to_thread(lambda: sqlite3.connect(":memory:").execute("SELECT 1"))


async def safe_unbound_class_method(session: Session):
    return await asyncio.to_thread(Session.execute, session, "SELECT 1")
"#,
        ),
    ]);

    let report = scan_path(&dir).unwrap();
    let _ = std::fs::remove_dir_all(&dir);

    assert!(
        report.findings.is_empty(),
        "must not emit findings: {:?}",
        report.findings
    );
    let pairs = advice_pairs(&report);
    for expected in [
        ("offload-stateful-session", "bad_sqlalchemy_to_thread"),
        ("offload-stateful-session", "bad_sqlmodel_partial"),
        ("offload-stateful-session", "bad_sqlite_conn_executor"),
        ("offload-stateful-session", "bad_sqlite_cursor_lambda"),
    ] {
        assert!(
            pairs.contains(&expected),
            "missing expected advice {expected:?}, got {pairs:?}"
        );
    }
    for banned in [
        "safe_unit_of_work",
        "safe_engine_connect",
        "safe_lambda_opens_own",
        "safe_unbound_class_method",
    ] {
        assert!(
            !pairs.iter().any(|(_, e)| *e == banned),
            "false-positive advice on {banned}: {pairs:?}"
        );
    }
    assert!(
        report
            .advice
            .iter()
            .filter(|a| a.rule == "offload-stateful-session")
            .all(|a| a.category == AdviceCategory::Correctness)
    );
}

#[test]
fn useless_local_async_lock_positives_and_negatives() {
    let dir = temp_project(&[
        (
            ".venv/pyvenv.cfg",
            "home = /usr/bin\nversion = 3.12.0\ninclude-system-site-packages = false\n",
        ),
        (
            ".venv/lib/python3.12/site-packages/anyio/__init__.py",
            "class Lock:\n    async def __aenter__(self):\n        return self\n    async def __aexit__(self, *args):\n        return False\n",
        ),
        (
            "app.py",
            r#"import asyncio
import anyio

MODULE_LOCK = asyncio.Lock()


async def bad_inline_lock():
    async with asyncio.Lock():
        await asyncio.sleep(0)


async def bad_inline_semaphore():
    async with asyncio.Semaphore(5):
        await asyncio.sleep(0)


async def bad_inline_anyio_lock():
    async with anyio.Lock():
        await asyncio.sleep(0)


async def bad_local_var_lock():
    lock = asyncio.Lock()
    async with lock:
        await asyncio.sleep(0)


async def safe_closure_shared_sem(items: list[int]):
    sem = asyncio.Semaphore(5)

    async def one(x: int) -> int:
        async with sem:
            await asyncio.sleep(0)
            return x

    return await asyncio.gather(*(one(x) for x in items))


async def _helper_with_lock(lock: asyncio.Lock):
    async with lock:
        await asyncio.sleep(0)


async def safe_passed_arg_lock():
    lock = asyncio.Lock()
    async with lock:
        await _helper_with_lock(lock)


class Service:
    def __init__(self) -> None:
        self.lock = asyncio.Lock()

    async def safe_instance_attr_lock(self) -> None:
        async with self.lock:
            await asyncio.sleep(0)


async def safe_module_lock():
    async with MODULE_LOCK:
        await asyncio.sleep(0)


async def safe_async_generator_lock():
    lock = asyncio.Lock()
    async with lock:
        yield 1
"#,
        ),
    ]);

    let report = scan_path(&dir).unwrap();
    let _ = std::fs::remove_dir_all(&dir);

    assert!(report.findings.is_empty(), "{:?}", report.findings);
    let pairs = advice_pairs(&report);
    for expected in [
        ("useless-local-async-lock", "bad_inline_lock"),
        ("useless-local-async-lock", "bad_inline_semaphore"),
        ("useless-local-async-lock", "bad_inline_anyio_lock"),
        ("useless-local-async-lock", "bad_local_var_lock"),
    ] {
        assert!(
            pairs.contains(&expected),
            "missing expected advice {expected:?}, got {pairs:?}"
        );
    }
    for banned in [
        "safe_closure_shared_sem",
        "safe_passed_arg_lock",
        "Service.safe_instance_attr_lock",
        "safe_module_lock",
        "safe_async_generator_lock",
    ] {
        assert!(
            !pairs.iter().any(|(_, e)| *e == banned),
            "false-positive advice on {banned}: {pairs:?}"
        );
    }
    assert!(
        report
            .advice
            .iter()
            .filter(|a| a.rule == "useless-local-async-lock")
            .all(|a| a.category == AdviceCategory::Correctness)
    );
}

#[test]
fn swallowed_cancelled_error_positives_and_negatives() {
    let dir = temp_project(&[(
        "app.py",
        r#"import asyncio
from asyncio import CancelledError
from contextlib import asynccontextmanager


class App:
    def get(self, _path: str):
        def deco(fn):
            return fn
        return deco


app = App()


async def bad_fallthrough_cancelled():
    try:
        await asyncio.sleep(0)
    except asyncio.CancelledError:
        pass


@app.get("/x")
async def bad_return_cancelled():
    try:
        await asyncio.sleep(0)
    except CancelledError:
        return None


async def bad_tuple_cancelled():
    try:
        await asyncio.sleep(0)
    except (ValueError, asyncio.CancelledError):
        return 0


async def bad_bare_except_over_await():
    try:
        await asyncio.sleep(0)
    except:
        return -1


async def bad_base_exception_over_await():
    try:
        await asyncio.sleep(0)
    except BaseException:
        pass


async def safe_reraise_cancelled():
    try:
        await asyncio.sleep(0)
    except asyncio.CancelledError:
        raise


async def safe_branch_both_raise(flag: bool):
    try:
        await asyncio.sleep(0)
    except asyncio.CancelledError as exc:
        if flag:
            raise RuntimeError("boom") from exc
        else:
            raise


async def safe_except_exception():
    try:
        await asyncio.sleep(0)
    except Exception:
        return None


async def safe_preceding_cancelled_handler():
    try:
        await asyncio.sleep(0)
    except asyncio.CancelledError:
        raise
    except BaseException:
        return None


async def safe_bare_except_sync_try_body(raw: str):
    try:
        return int(raw)
    except:
        return 0


async def safe_uncancel_guard():
    try:
        await asyncio.sleep(0)
    except asyncio.CancelledError:
        task = asyncio.current_task()
        if task is not None and task.uncancel() == 0:
            raise
        return None


async def safe_background_task_entry():
    try:
        await asyncio.sleep(1)
    except asyncio.CancelledError:
        return None


async def spawn_bg(tasks: set):
    t = asyncio.create_task(safe_background_task_entry())
    tasks.add(t)


@asynccontextmanager
async def lifespan(_app):
    try:
        yield
    except asyncio.CancelledError:
        return
"#,
    )]);

    let report = scan_path(&dir).unwrap();
    let _ = std::fs::remove_dir_all(&dir);

    assert!(report.findings.is_empty(), "{:?}", report.findings);
    let pairs = advice_pairs(&report);
    for expected in [
        ("swallowed-cancelled-error", "bad_fallthrough_cancelled"),
        ("swallowed-cancelled-error", "bad_return_cancelled"),
        ("swallowed-cancelled-error", "bad_tuple_cancelled"),
        ("swallowed-cancelled-error", "bad_bare_except_over_await"),
        ("swallowed-cancelled-error", "bad_base_exception_over_await"),
    ] {
        assert!(
            pairs.contains(&expected),
            "missing expected advice {expected:?}, got {pairs:?}"
        );
    }
    for banned in [
        "safe_reraise_cancelled",
        "safe_branch_both_raise",
        "safe_except_exception",
        "safe_preceding_cancelled_handler",
        "safe_bare_except_sync_try_body",
        "safe_uncancel_guard",
        "safe_background_task_entry",
        "lifespan",
    ] {
        assert!(
            !pairs.iter().any(|(_, e)| *e == banned),
            "false-positive advice on {banned}: {pairs:?}"
        );
    }
    assert!(
        report
            .advice
            .iter()
            .filter(|a| a.rule == "swallowed-cancelled-error")
            .all(|a| a.category == AdviceCategory::Correctness)
    );
}
