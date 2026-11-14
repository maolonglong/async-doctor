//! E2E tests for async anti-pattern rules and expanded async twins:
//! - `sync-with-async-cm`, `sync-for-async-iterable`, `eager-offload-call`, `unawaited-coroutine` (findings)
//! - `dangling-task`, `ephemeral-async-client`, `shared-async-session`, `offload-has-async-twin` (advice)

use std::path::Path;
use std::process::Command;

fn bin() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_async-doctor"));
    cmd.env_remove("VIRTUAL_ENV").env_remove("CONDA_PREFIX");
    // Isolated cache: caller XDG_CACHE_HOME / prior runs must not affect results.
    let cache = std::env::temp_dir().join(format!(
        "async-doctor-test-cache-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::create_dir_all(&cache);
    cmd.env("XDG_CACHE_HOME", cache);
    cmd
}

fn init_fake_venv(root: &Path) -> std::path::PathBuf {
    let sp = root.join(".venv/lib/python3.12/site-packages");
    std::fs::create_dir_all(&sp).expect("create site-packages");
    std::fs::write(
        root.join(".venv/pyvenv.cfg"),
        "home = /usr/bin\nversion = 3.12.0\n",
    )
    .expect("write pyvenv.cfg");
    sp
}

fn add_pkg(sp: &Path, dist_name: &str, rel_file: &str, content: &str) {
    let target = sp.join(rel_file);
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).expect("create pkg parent");
    }
    std::fs::write(&target, content).expect("write pkg file");
    std::fs::create_dir_all(sp.join(format!("{dist_name}-1.0.0.dist-info"))).expect("dist-info");
}

#[test]
fn sync_with_async_cm_reports_finding_and_respects_dual_cm_and_async_with() {
    let dir = tempfile::tempdir().expect("tempdir");
    let py = dir.path().join("app.py");
    std::fs::write(
        &py,
        concat!(
            "import asyncio\n\n",
            "class OnlyAsyncCM:\n",
            "    async def __aenter__(self):\n",
            "        return self\n",
            "    async def __aexit__(self, exc_type, exc, tb):\n",
            "        return False\n\n",
            "class DualCM:\n",
            "    def __enter__(self):\n",
            "        return self\n",
            "    def __exit__(self, exc_type, exc, tb):\n",
            "        return False\n",
            "    async def __aenter__(self):\n",
            "        return self\n",
            "    async def __aexit__(self, exc_type, exc, tb):\n",
            "        return False\n\n",
            "async def bad_stdlib_lock():\n",
            "    with asyncio.Lock():\n",
            "        pass\n\n",
            "async def bad_project_cm():\n",
            "    with OnlyAsyncCM():\n",
            "        pass\n\n",
            "async def good_cases():\n",
            "    async with asyncio.Lock():\n",
            "        pass\n",
            "    async with OnlyAsyncCM():\n",
            "        pass\n",
            "    with DualCM():\n",
            "        pass\n",
        ),
    )
    .expect("write app.py");

    let out = bin()
        .args(["--format", "json"])
        .arg(dir.path())
        .output()
        .expect("run async-doctor");
    assert_eq!(out.status.code(), Some(1));

    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("parse json");
    let findings = v["findings"].as_array().expect("findings array");
    let rules: Vec<&str> = findings.iter().filter_map(|f| f["rule"].as_str()).collect();
    assert_eq!(
        rules,
        vec!["sync-with-async-cm", "sync-with-async-cm"],
        "expected exactly 2 sync-with-async-cm findings, got: {findings:#?}"
    );
    let entries: Vec<&str> = findings
        .iter()
        .filter_map(|f| f["entry"]["name"].as_str())
        .collect();
    assert!(entries.contains(&"bad_stdlib_lock"));
    assert!(entries.contains(&"bad_project_cm"));
    assert!(!entries.contains(&"good_cases"));
}

#[test]
fn dangling_task_reports_correctness_advice_only_when_discarded() {
    let dir = tempfile::tempdir().expect("tempdir");
    let py = dir.path().join("app.py");
    std::fs::write(
        &py,
        concat!(
            "import asyncio\n\n",
            "async def bg():\n",
            "    await asyncio.sleep(0)\n\n",
            "async def bad_fire_and_forget():\n",
            "    asyncio.create_task(bg())\n\n",
            "async def good_kept_or_awaited():\n",
            "    t = asyncio.create_task(bg())\n",
            "    await t\n",
            "    await asyncio.create_task(bg())\n",
            "    tasks = []\n",
            "    tasks.append(asyncio.create_task(bg()))\n",
            "    async with asyncio.TaskGroup() as tg:\n",
            "        tg.create_task(bg())\n",
            "    return asyncio.create_task(bg())\n",
        ),
    )
    .expect("write app.py");

    let out = bin()
        .args(["--format", "json"])
        .arg(dir.path())
        .output()
        .expect("run async-doctor");
    assert_eq!(
        out.status.code(),
        Some(0),
        "dangling-task is advice and must not fail exit code"
    );

    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("parse json");
    assert!(v["findings"].as_array().unwrap().is_empty());
    let advice = v["advice"].as_array().expect("advice array");
    assert_eq!(
        advice.len(),
        1,
        "expected exactly 1 dangling-task advice item, got: {advice:#?}"
    );
    assert_eq!(advice[0]["rule"], "dangling-task");
    assert_eq!(advice[0]["category"], "correctness");
    assert_eq!(advice[0]["entry"]["name"], "bad_fire_and_forget");
}

#[test]
fn explain_supports_all_async_anti_pattern_rules() {
    for rule in [
        "sync-with-async-cm",
        "sync-for-async-iterable",
        "eager-offload-call",
        "unawaited-coroutine",
        "dangling-task",
        "ephemeral-async-client",
        "shared-async-session",
    ] {
        let out = bin().args(["explain", rule]).output().expect("run explain");
        assert_eq!(out.status.code(), Some(0), "explain failed for {rule}");
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(stdout.contains(rule));
        assert!(stdout.contains(&format!("#{rule}")));
        if rule == "unawaited-coroutine" {
            assert!(
                stdout.contains("for x in await") && stdout.contains("with await"),
                "explain must show awaiting the result before protocol use: {stdout}"
            );
        }
    }
}

#[test]
fn anyio_fail_after_and_move_on_after_are_not_sync_with_async_cm() {
    let dir = tempfile::tempdir().expect("tempdir");
    let py = dir.path().join("app.py");
    std::fs::write(
        &py,
        concat!(
            "import anyio\n\n",
            "async def ok_timeouts():\n",
            "    with anyio.fail_after(1.0):\n",
            "        await anyio.sleep(0)\n",
            "    with anyio.move_on_after(1.0):\n",
            "        await anyio.sleep(0)\n\n",
            "async def bad_lock():\n",
            "    with anyio.Lock():\n",
            "        pass\n",
        ),
    )
    .expect("write app.py");

    let sp = init_fake_venv(dir.path());
    add_pkg(
        &sp,
        "anyio",
        "anyio/__init__.py",
        concat!(
            "class CancelScope:\n",
            "    def __enter__(self):\n",
            "        return self\n",
            "    def __exit__(self, *args):\n",
            "        return False\n\n",
            "def fail_after(delay, shield=False, reason=None):\n",
            "    return CancelScope()\n\n",
            "def move_on_after(delay, shield=False):\n",
            "    return CancelScope()\n\n",
            "class Lock:\n",
            "    async def __aenter__(self):\n",
            "        return self\n",
            "    async def __aexit__(self, *args):\n",
            "        return False\n\n",
            "async def sleep(seconds):\n",
            "    return None\n",
        ),
    );

    let out = bin()
        .args(["--format", "json"])
        .arg(dir.path())
        .output()
        .expect("run async-doctor");
    assert_eq!(out.status.code(), Some(1));

    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("parse json");
    let findings = v["findings"].as_array().expect("findings");
    assert_eq!(findings.len(), 1, "got: {findings:#?}");
    assert_eq!(findings[0]["entry"]["name"], "bad_lock");
}

#[test]
fn sync_for_async_iterable_catches_for_and_comp_and_respects_dual_and_getitem() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        dir.path().join("app.py"),
        concat!(
            "async def agen():\n",
            "    yield 1\n\n",
            "class OnlyAsyncIter:\n",
            "    def __aiter__(self):\n",
            "        return self\n",
            "    async def __anext__(self):\n",
            "        raise StopAsyncIteration\n\n",
            "class DualIter:\n",
            "    def __iter__(self):\n",
            "        return iter([1])\n",
            "    def __aiter__(self):\n",
            "        return self\n\n",
            "class LegacyGetItemIter:\n",
            "    def __getitem__(self, idx):\n",
            "        raise IndexError\n",
            "    def __aiter__(self):\n",
            "        return self\n\n",
            "async def bad_for_agen():\n",
            "    for _x in agen():\n",
            "        pass\n\n",
            "async def bad_comp_agen():\n",
            "    return [x for x in agen()]\n\n",
            "async def bad_for_cls():\n",
            "    it = OnlyAsyncIter()\n",
            "    for _x in it:\n",
            "        pass\n\n",
            "async def good_cases():\n",
            "    async for _x in agen():\n",
            "        pass\n",
            "    _ = [x async for x in agen()]\n",
            "    for _x in DualIter():\n",
            "        pass\n",
            "    for _x in LegacyGetItemIter():\n",
            "        pass\n",
        ),
    )
    .expect("write app.py");

    let out = bin()
        .args(["--format", "json"])
        .arg(dir.path())
        .output()
        .expect("run");
    assert_eq!(out.status.code(), Some(1));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("json");
    let findings = v["findings"].as_array().unwrap();
    let entries: Vec<&str> = findings
        .iter()
        .filter(|f| f["rule"] == "sync-for-async-iterable")
        .filter_map(|f| f["entry"]["name"].as_str())
        .collect();
    assert_eq!(entries.len(), 3, "got: {findings:#?}");
    assert!(entries.contains(&"bad_for_agen"));
    assert!(entries.contains(&"bad_comp_agen"));
    assert!(entries.contains(&"bad_for_cls"));
    assert!(!entries.contains(&"good_cases"));
}

#[test]
fn eager_offload_call_catches_non_callable_result_and_dedupes_blocking_sink() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        dir.path().join("app.py"),
        concat!(
            "import asyncio\n",
            "import functools\n",
            "import time\n\n",
            "def pure_math(x: int) -> int:\n",
            "    return x * 2\n\n",
            "def mutate_only(d: dict) -> None:\n",
            "    d['ok'] = True\n\n",
            "def make_callable(x: int):\n",
            "    return lambda: x + 1\n\n",
            "def blocking_work():\n",
            "    time.sleep(0.1)\n\n",
            "class NonCallableObj:\n",
            "    pass\n\n",
            "class CallableObj:\n",
            "    def __call__(self):\n",
            "        return 1\n\n",
            "async def bad_scalar():\n",
            "    await asyncio.to_thread(pure_math(21))\n\n",
            "async def bad_none():\n",
            "    await asyncio.to_thread(mutate_only({}))\n\n",
            "async def bad_instance():\n",
            "    await asyncio.to_thread(NonCallableObj())\n\n",
            "async def bad_blocking_deduped():\n",
            "    await asyncio.to_thread(blocking_work())\n\n",
            "async def good_offloads():\n",
            "    await asyncio.to_thread(pure_math, 21)\n",
            "    await asyncio.to_thread(functools.partial(pure_math, 21))\n",
            "    await asyncio.to_thread(make_callable(21))\n",
            "    await asyncio.to_thread(CallableObj())\n",
        ),
    )
    .expect("write app.py");

    let out = bin()
        .args(["--format", "json"])
        .arg(dir.path())
        .output()
        .expect("run");
    assert_eq!(out.status.code(), Some(1));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("json");
    let findings = v["findings"].as_array().unwrap();
    let eager_entries: Vec<&str> = findings
        .iter()
        .filter(|f| f["rule"] == "eager-offload-call")
        .filter_map(|f| f["entry"]["name"].as_str())
        .collect();
    assert_eq!(eager_entries.len(), 3, "got: {findings:#?}");
    assert!(eager_entries.contains(&"bad_scalar"));
    assert!(eager_entries.contains(&"bad_none"));
    assert!(eager_entries.contains(&"bad_instance"));
    // `blocking_work()` already emits `blocking-sleep` on that offload site — no double report.
    assert!(!eager_entries.contains(&"bad_blocking_deduped"));
    assert!(
        findings
            .iter()
            .any(|f| f["entry"]["name"] == "bad_blocking_deduped" && f["rule"] == "blocking-sleep")
    );
    assert!(
        !findings
            .iter()
            .any(|f| f["entry"]["name"] == "good_offloads")
    );
}

#[test]
fn unawaited_coroutine_catches_bare_coro_calls_and_skips_generators_and_wrappers() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        dir.path().join("app.py"),
        concat!(
            "import asyncio\n",
            "from contextlib import asynccontextmanager\n\n",
            "def background_wrapper(fn):\n",
            "    return fn\n\n",
            "async def send_email(addr: str) -> None:\n",
            "    await asyncio.sleep(0)\n\n",
            "async def stream_items():\n",
            "    yield 1\n\n",
            "@asynccontextmanager\n",
            "async def managed_resource():\n",
            "    yield 1\n\n",
            "@background_wrapper\n",
            "async def fire_job():\n",
            "    await asyncio.sleep(0)\n\n",
            "async def bad_unawaited():\n",
            "    send_email('a@example.com')\n",
            "    asyncio.sleep(1)\n\n",
            "async def good_uses():\n",
            "    await send_email('a@example.com')\n",
            "    coro = send_email('b@example.com')\n",
            "    await coro\n",
            "    stream_items()\n",
            "    managed_resource()\n",
            "    fire_job()\n",
            "    return send_email('c@example.com')\n",
        ),
    )
    .expect("write app.py");

    let out = bin()
        .args(["--format", "json"])
        .arg(dir.path())
        .output()
        .expect("run");
    assert_eq!(out.status.code(), Some(1));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("json");
    let findings = v["findings"].as_array().unwrap();
    assert_eq!(findings.len(), 2, "got: {findings:#?}");
    assert!(
        findings.iter().all(|f| {
            f["rule"] == "unawaited-coroutine" && f["entry"]["name"] == "bad_unawaited"
        })
    );
}

#[test]
fn ephemeral_async_client_and_shared_async_session_advice_precision() {
    let dir = tempfile::tempdir().expect("tempdir");
    let sp = init_fake_venv(dir.path());
    add_pkg(
        &sp,
        "httpx",
        "httpx/__init__.py",
        concat!(
            "class AsyncClient:\n",
            "    def __init__(self, **kwargs):\n",
            "        pass\n",
            "    async def __aenter__(self):\n",
            "        return self\n",
            "    async def __aexit__(self, *args):\n",
            "        return False\n",
            "    async def get(self, url: str):\n",
            "        return None\n",
            "    async def aclose(self):\n",
            "        return None\n",
        ),
    );
    add_pkg(
        &sp,
        "SQLAlchemy",
        "sqlalchemy/ext/asyncio/__init__.py",
        concat!(
            "class AsyncSession:\n",
            "    async def __aenter__(self):\n",
            "        return self\n",
            "    async def __aexit__(self, *args):\n",
            "        return False\n",
            "    async def execute(self, q: str):\n",
            "        return None\n\n",
            "class async_sessionmaker:\n",
            "    def __call__(self) -> AsyncSession:\n",
            "        return AsyncSession()\n",
        ),
    );
    std::fs::write(sp.join("sqlalchemy/__init__.py"), "").unwrap();
    std::fs::write(sp.join("sqlalchemy/ext/__init__.py"), "").unwrap();

    std::fs::write(
        dir.path().join("app.py"),
        concat!(
            "import asyncio\n",
            "from contextlib import asynccontextmanager\n",
            "import httpx\n",
            "from sqlalchemy.ext.asyncio import AsyncSession, async_sessionmaker\n\n",
            "class App:\n",
            "    def get(self, _path: str):\n",
            "        def deco(fn):\n",
            "            return fn\n",
            "        return deco\n\n",
            "app = App()\n\n",
            "async def helper_fetch():\n",
            "    async with httpx.AsyncClient() as c:\n",
            "        await c.get('https://example.com')\n\n",
            "@app.get('/users')\n",
            "async def bad_route_via_helper():\n",
            "    await helper_fetch()\n\n",
            "@app.get('/manual')\n",
            "async def bad_route_manual_close():\n",
            "    c = httpx.AsyncClient()\n",
            "    await c.get('https://example.com')\n",
            "    await c.aclose()\n\n",
            "async def bad_loop_client(urls: list[str]):\n",
            "    for u in urls:\n",
            "        async with httpx.AsyncClient() as c:\n",
            "            await c.get(u)\n\n",
            "@asynccontextmanager\n",
            "async def lifespan(_app):\n",
            "    async with httpx.AsyncClient() as c:\n",
            "        yield c\n\n",
            "async def ok_cli_oneshot(urls: list[str]):\n",
            "    c = httpx.AsyncClient()\n",
            "    for u in urls:\n",
            "        await c.get(u)\n",
            "    await c.aclose()\n",
            "    async with httpx.AsyncClient(transport='mock') as c2:\n",
            "        await c2.get('https://example.com')\n\n",
            "async def run_q(s: AsyncSession, q: str):\n",
            "    return await s.execute(q)\n\n",
            "async def run_with_maker(maker: async_sessionmaker, q: str):\n",
            "    async with maker() as s:\n",
            "        return await s.execute(q)\n\n",
            "async def bad_shared_gather(session: AsyncSession):\n",
            "    await asyncio.gather(run_q(session, 'SELECT 1'), run_q(session, 'SELECT 2'))\n\n",
            "async def bad_shared_taskgroup(session: AsyncSession, qs: list[str]):\n",
            "    async with asyncio.TaskGroup() as tg:\n",
            "        for q in qs:\n",
            "            tg.create_task(run_q(session, q))\n\n",
            "async def ok_sessions(s1: AsyncSession, s2: AsyncSession, maker: async_sessionmaker):\n",
            "    await asyncio.gather(run_q(s1, 'SELECT 1'), run_q(s2, 'SELECT 2'))\n",
            "    await asyncio.gather(run_with_maker(maker, 'SELECT 1'), run_with_maker(maker, 'SELECT 2'))\n",
        ),
    )
    .expect("write app.py");

    let out = bin()
        .args(["--format", "json"])
        .arg(dir.path())
        .output()
        .expect("run");
    assert_eq!(out.status.code(), Some(0), "advice must exit 0");
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("json");
    assert!(v["findings"].as_array().unwrap().is_empty());
    let advice = v["advice"].as_array().unwrap();

    let ephemeral: Vec<&str> = advice
        .iter()
        .filter(|a| a["rule"] == "ephemeral-async-client")
        .filter_map(|a| a["entry"]["name"].as_str())
        .collect();
    assert_eq!(ephemeral.len(), 3, "got advice: {advice:#?}");
    assert!(ephemeral.contains(&"bad_route_via_helper"));
    assert!(ephemeral.contains(&"bad_route_manual_close"));
    assert!(ephemeral.contains(&"bad_loop_client"));

    let shared: Vec<&str> = advice
        .iter()
        .filter(|a| a["rule"] == "shared-async-session")
        .filter_map(|a| a["entry"]["name"].as_str())
        .collect();
    assert_eq!(shared.len(), 2, "got advice: {advice:#?}");
    assert!(shared.contains(&"bad_shared_gather"));
    assert!(shared.contains(&"bad_shared_taskgroup"));
}

#[test]
fn expanded_async_twins_gate_sqlalchemy_on_greenlet_and_driver_and_support_grpc_aio() {
    let dir = tempfile::tempdir().expect("tempdir");
    let sp = init_fake_venv(dir.path());
    add_pkg(
        &sp,
        "grpcio",
        "grpc/__init__.py",
        "class Channel:\n    pass\ndef insecure_channel(target: str) -> Channel:\n    return Channel()\n",
    );
    add_pkg(
        &sp,
        "grpcio",
        "grpc/aio/__init__.py",
        "def insecure_channel(target: str): pass\n",
    );
    add_pkg(
        &sp,
        "SQLAlchemy",
        "sqlalchemy/orm/__init__.py",
        "class Session:\n    def execute(self, stmt: str):\n        return None\n",
    );
    std::fs::write(sp.join("sqlalchemy/__init__.py"), "").unwrap();
    std::fs::create_dir_all(sp.join("sqlalchemy/ext/asyncio")).unwrap();
    std::fs::write(sp.join("sqlalchemy/ext/__init__.py"), "").unwrap();
    std::fs::write(
        sp.join("sqlalchemy/ext/asyncio/__init__.py"),
        "class AsyncSession: pass\nclass AsyncEngine: pass\n",
    )
    .unwrap();
    add_pkg(&sp, "greenlet", "greenlet.py", "");

    std::fs::write(
        dir.path().join("app.py"),
        concat!(
            "import asyncio\n",
            "import grpc\n",
            "from sqlalchemy.orm import Session\n\n",
            "def open_rpc():\n",
            "    return grpc.insecure_channel('localhost:50051')\n\n",
            "def run_sql(s: Session):\n",
            "    return s.execute('SELECT 1')\n\n",
            "async def entry(s: Session):\n",
            "    await asyncio.to_thread(open_rpc)\n",
            "    await asyncio.to_thread(run_sql, s)\n",
        ),
    )
    .expect("write app.py");

    // 1. Without an async SQL driver (`any_needs`), grpc.aio fires but SQLAlchemy stays silent.
    let out1 = bin()
        .args(["--format", "json"])
        .arg(dir.path())
        .output()
        .expect("run 1");
    assert_eq!(out1.status.code(), Some(0));
    let v1: serde_json::Value = serde_json::from_slice(&out1.stdout).unwrap();
    let hints1: Vec<&str> = v1["advice"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|a| a["fix"]["hint"].as_str())
        .collect();
    assert_eq!(
        hints1.len(),
        1,
        "expected only grpc.aio advice: {hints1:#?}"
    );
    assert!(hints1[0].contains("grpc.aio"));

    // 2. Once `asyncpg` is installed, SQLAlchemy AsyncSession twin advice fires too!
    add_pkg(&sp, "asyncpg", "asyncpg/__init__.py", "");
    let out2 = bin()
        .args(["--format", "json"])
        .arg(dir.path())
        .output()
        .expect("run 2");
    let v2: serde_json::Value = serde_json::from_slice(&out2.stdout).unwrap();
    let hints2: Vec<&str> = v2["advice"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|a| a["fix"]["hint"].as_str())
        .collect();
    assert_eq!(
        hints2.len(),
        2,
        "expected grpc.aio + AsyncSession advice: {hints2:#?}"
    );
    assert!(
        hints2
            .iter()
            .any(|h| h.contains("sqlalchemy.ext.asyncio.AsyncSession"))
    );
}

#[test]
fn inline_suppressions_and_why_work_for_new_anti_pattern_rules() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        dir.path().join("app.py"),
        concat!(
            "import asyncio\n\n",
            "async def ping() -> None:\n",
            "    await asyncio.sleep(0)\n\n",
            "async def agen():\n",
            "    yield 1\n\n",
            "def scalar() -> int:\n",
            "    return 7\n\n",
            "async def suppressed_all():\n",
            "    ping()  # async-doctor: ignore[unawaited-coroutine]: intentional test\n",
            "    for _ in agen():  # async-doctor: ignore[sync-for-async-iterable]: intentional test\n",
            "        pass\n",
            "    await asyncio.to_thread(scalar())  # async-doctor: ignore[eager-offload-call]: intentional test\n\n",
            "async def unsuppressed_coro():\n",
            "    ping()\n",
        ),
    )
    .expect("write app.py");

    let out = bin()
        .args(["--format", "json"])
        .arg(dir.path())
        .output()
        .expect("run");
    assert_eq!(out.status.code(), Some(1));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(
        v["unusedSuppressions"].as_array().unwrap().is_empty(),
        "expected 0 unused suppressions: {v:#?}"
    );
    let findings = v["findings"].as_array().unwrap();
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0]["entry"]["name"], "unsuppressed_coro");
    assert_eq!(findings[0]["rule"], "unawaited-coroutine");

    let why_out = bin()
        .args(["why", "app.py:19", "--path"])
        .arg(dir.path())
        .output()
        .expect("why");
    // `why` returns EXIT_FINDINGS (1) when findings match the queried location.
    assert_eq!(why_out.status.code(), Some(1));
    let why_stdout = String::from_utf8_lossy(&why_out.stdout);
    assert!(
        why_stdout.contains("unawaited-coroutine"),
        "got: {why_stdout}"
    );
}
