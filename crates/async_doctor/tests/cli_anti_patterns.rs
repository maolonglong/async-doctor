//! E2E tests for async anti-pattern rules:
//! - `sync-with-async-cm` (finding)
//! - `dangling-task` (correctness advice)

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
fn explain_supports_sync_with_async_cm_and_dangling_task() {
    for rule in ["sync-with-async-cm", "dangling-task"] {
        let out = bin().args(["explain", rule]).output().expect("run explain");
        assert_eq!(out.status.code(), Some(0));
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(stdout.contains(rule));
        assert!(stdout.contains(&format!("#{rule}")));
    }
}

#[test]
fn anyio_fail_after_and_move_on_after_are_not_sync_with_async_cm() {
    // Verified against anyio 4.x: fail_after is a sync @contextmanager and
    // move_on_after returns a CancelScope with __enter__ (no __aenter__).
    // `with anyio.fail_after(...)` / `with anyio.move_on_after(...)` is correct;
    // suggesting `async with` would break callers.
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

    // Minimal anyio stub so resolution does not require a real install. The
    // known-qualname allowlist still applies to External callees; Lock must
    // keep flagging while fail_after / move_on_after must stay silent.
    let sp = dir
        .path()
        .join(".venv/lib/python3.12/site-packages/anyio");
    std::fs::create_dir_all(&sp).expect("anyio stub dir");
    std::fs::write(
        sp.join("__init__.py"),
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
    )
    .expect("write anyio stub");
    std::fs::write(
        dir.path().join(".venv/pyvenv.cfg"),
        "home = /usr/bin\nversion = 3.12.0\n",
    )
    .expect("pyvenv.cfg");
    std::fs::create_dir_all(sp.parent().unwrap().join("anyio-4.0.0.dist-info")).unwrap();

    let out = bin()
        .args(["--format", "json"])
        .arg(dir.path())
        .output()
        .expect("run async-doctor");
    assert_eq!(
        out.status.code(),
        Some(1),
        "stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );

    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("parse json");
    let findings = v["findings"].as_array().expect("findings");
    let entries: Vec<&str> = findings
        .iter()
        .filter_map(|f| f["entry"]["name"].as_str())
        .collect();
    assert!(
        entries.contains(&"bad_lock"),
        "anyio.Lock remains async-only: {findings:#?}"
    );
    assert!(
        !entries.contains(&"ok_timeouts"),
        "fail_after/move_on_after must not flag sync-with-async-cm: {findings:#?}"
    );
    assert_eq!(
        findings.len(),
        1,
        "expected only the Lock finding, got: {findings:#?}"
    );
}
