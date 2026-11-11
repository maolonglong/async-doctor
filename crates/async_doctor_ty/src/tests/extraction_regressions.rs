//! End-to-end extraction regression tests for CallCollector scoping,
//! bound-method parameter offsets, lambda guards, Union narrowing,
//! constructor/super resolution, and offload argument handling.

use std::fs;

use crate::scan_path;
use crate::tests::helpers::temp_project;

#[test]
fn bound_method_param_guard_and_method_callback_end_to_end() {
    let dir = temp_project(&[(
        "app.py",
        r#"
import time

def blocker() -> None:
    time.sleep(1)

class Service:
    def maybe_sleep(self, keep_alive: bool, should_block: bool) -> None:
        if should_block:
            time.sleep(1)

    def run_callback(self, cb) -> None:
        cb()

async def route_ok_and_cb() -> None:
    svc = Service()
    svc.maybe_sleep(True, False)
    svc.run_callback(blocker)
"#,
    )]);
    let report = scan_path(&dir).unwrap();
    let _ = fs::remove_dir_all(&dir);

    assert!(
        report.candidates.is_empty(),
        "svc.maybe_sleep(True, False) is provably non-blocking: {:?}",
        report.candidates
    );
    assert_eq!(
        report.findings.len(),
        1,
        "only svc.run_callback(blocker) must fire: {:?}",
        report.findings
    );
    assert!(
        report.findings[0]
            .chain
            .iter()
            .any(|s| s.text.contains("run_callback"))
    );
}

#[test]
fn async_with_receiver_does_not_leak_to_subsequent_function() {
    let dir = temp_project(&[
        (
            ".venv/pyvenv.cfg",
            "home = /usr/bin\ninclude-system-site-packages = false\nversion = 3.12.0\n",
        ),
        (
            ".venv/lib/python3.12/site-packages/redis/__init__.py",
            r#"
from .client import Redis
"#,
        ),
        (
            ".venv/lib/python3.12/site-packages/redis/client.py",
            r#"
class Pipeline:
    def set(self, k: str, v: str) -> "Pipeline":
        return self
    def execute(self) -> list:
        return []

class Redis:
    def pipeline(self) -> Pipeline:
        return Pipeline()
"#,
        ),
        (
            ".venv/lib/python3.12/site-packages/redis/asyncio/__init__.py",
            r#"
from .client import Redis
"#,
        ),
        (
            ".venv/lib/python3.12/site-packages/redis/asyncio/client.py",
            r#"
class Pipeline:
    async def __aenter__(self) -> "Pipeline":
        return self
    async def __aexit__(self, *args) -> None:
        return None
    def set(self, k: str, v: str) -> "Pipeline":
        return self
    async def execute(self) -> list:
        return []

class Redis:
    def pipeline(self) -> Pipeline:
        return Pipeline()
"#,
        ),
        (
            "app.py",
            r#"
import redis
import redis.asyncio

async def first_async_ok(r: redis.asyncio.Redis) -> None:
    async with r.pipeline() as pipe:
        pipe.set("k", "v")
        await pipe.execute()

async def second_sync_blocks(pipe: redis.client.Pipeline) -> None:
    # Parameter `pipe` must not inherit `redis.asyncio.client.Pipeline`
    # from `first_async_ok`'s `async with ... as pipe`.
    pipe.set("k", "v")
    pipe.execute()
"#,
        ),
    ]);
    let report = crate::hermetic_env::with_clean_venv_env(|| scan_path(&dir).unwrap());
    let _ = fs::remove_dir_all(&dir);

    let names: Vec<&str> = report
        .findings
        .iter()
        .map(|f| f.entry.name.as_str())
        .collect();
    assert_eq!(
        names,
        vec!["second_sync_blocks"],
        "async_with_receivers leaked across functions: {:?}",
        report.findings
    );
}

#[test]
fn inline_local_lambda_preserves_enclosing_guards() {
    let dir = temp_project(&[(
        "app.py",
        r#"
import asyncio
import time

async def coro() -> None:
    await asyncio.sleep(0)

async def guarded_loop_ok() -> None:
    try:
        asyncio.get_running_loop()
    except RuntimeError:
        runner = lambda: asyncio.run(coro())
        runner()

async def branched_lambda(flag: bool) -> None:
    work = lambda: time.sleep(1)
    if flag:
        work()
"#,
    )]);
    let report = scan_path(&dir).unwrap();
    let _ = fs::remove_dir_all(&dir);

    assert!(
        report
            .findings
            .iter()
            .all(|f| f.entry.name != "guarded_loop_ok"),
        "lambda called inside except RuntimeError must preserve NoRunningLoopGuard: {:?}",
        report.findings
    );
    let branched = report
        .findings
        .iter()
        .find(|f| f.entry.name == "branched_lambda")
        .expect("branched_lambda finding");
    assert!(
        branched.conditions.iter().any(|c| c == "async-branch"),
        "expected async-branch on inline lambda under if flag, got: {:?}",
        branched.conditions
    );
}

#[test]
fn nested_narrowing_restores_outer_and_unnarrowed_union_does_not_guess_sync() {
    let dir = temp_project(&[(
        "app.py",
        r#"
from __future__ import annotations
import time

class SyncClient:
    def fetch(self) -> None:
        time.sleep(1)

class AsyncClient:
    async def fetch(self) -> None:
        return None

async def unnarrowed_union_ok(client: SyncClient | AsyncClient) -> None:
    await client.fetch()

async def outer_narrowing_preserved(x: object, flag: bool) -> None:
    if isinstance(x, AsyncClient):
        if flag and isinstance(x, SyncClient):
            pass
        await x.fetch()
"#,
    )]);
    let report = scan_path(&dir).unwrap();
    let _ = fs::remove_dir_all(&dir);

    assert!(
        report.findings.is_empty(),
        "unnarrowed union and restored outer AsyncClient narrowing must not emit findings: {:?}",
        report.findings
    );
}

#[test]
fn non_dataclass_post_init_two_arg_super_and_local_classes_behave_accurately() {
    let dir = temp_project(&[(
        "app.py",
        r#"
from dataclasses import dataclass
import time

class PlainNotDataclass:
    def __post_init__(self) -> None:
        time.sleep(1)

@dataclass
class RealDataclass:
    x: int = 1
    def __post_init__(self) -> None:
        time.sleep(1)

class GrandParent:
    def run(self) -> None:
        time.sleep(1)

class Parent(GrandParent):
    def run(self) -> None:
        return None

class Child(Parent):
    def run(self) -> None:
        super(Parent, self).run()

def first_scope() -> None:
    class Base:
        def work(self) -> None:
            time.sleep(1)
    class Local(Base):
        def trigger(self) -> None:
            super().work()

async def second_scope_ok() -> None:
    class SafeBase:
        def work(self) -> None:
            return None
    class Local(SafeBase):
        def trigger(self) -> None:
            super().work()
    Local().trigger()

async def plain_ok() -> None:
    PlainNotDataclass()

async def dataclass_bad() -> None:
    RealDataclass()

async def two_arg_super_bad() -> None:
    Child().run()
"#,
    )]);
    let report = scan_path(&dir).unwrap();
    let _ = fs::remove_dir_all(&dir);

    let mut entries: Vec<&str> = report
        .findings
        .iter()
        .map(|f| f.entry.name.as_str())
        .collect();
    entries.sort_unstable();
    assert_eq!(
        entries,
        vec!["dataclass_bad", "two_arg_super_bad"],
        "unexpected findings: {:?}",
        report.findings
    );
}

#[test]
fn offload_partial_visits_eager_args_on_event_loop() {
    let dir = temp_project(&[(
        "app.py",
        r#"
import asyncio
import functools

def consume(data: str) -> int:
    return len(data)

async def partial_eager_blocks() -> None:
    await asyncio.to_thread(functools.partial(consume, open("payload.txt").read()))
"#,
    )]);
    let report = scan_path(&dir).unwrap();
    let _ = fs::remove_dir_all(&dir);

    assert_eq!(
        report.findings.len(),
        1,
        "eager open().read() inside partial(...) must fire: {:?}",
        report.findings
    );
    assert_eq!(report.findings[0].rule, "blocking-file");
    assert!(
        report.findings[0].fix.hint.contains("pass the callable"),
        "expected eager offload hint, got: {}",
        report.findings[0].fix.hint
    );
}

#[test]
fn src_layout_failed_first_party_import_classified_as_unresolved_import() {
    let dir = temp_project(&[
        ("src/myapp/__init__.py", ""),
        (
            "src/myapp/routes.py",
            r#"
import myapp.missing_submodule

async def handler() -> None:
    return None
"#,
        ),
    ]);
    let facts = crate::extract_path(&dir).unwrap();
    let _ = fs::remove_dir_all(&dir);

    assert!(
        facts
            .unresolved_imports
            .iter()
            .any(|u| u.module == "myapp.missing_submodule"),
        "expected myapp.missing_submodule in first-party unresolved_imports, got unresolved_imports={:?} unresolved_third_party={:?}",
        facts.unresolved_imports,
        facts.unresolved_third_party_modules
    );
    assert!(
        !facts
            .unresolved_third_party_modules
            .iter()
            .any(|m| m == "myapp"),
        "src/ layout package myapp must not be recorded as missing third-party: {:?}",
        facts.unresolved_third_party_modules
    );
}
