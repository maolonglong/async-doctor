//! End-to-end regression tests for Wave-2 cross-crate seams:
//! - `--python <PATH>` across CLI scan, `scan_cache`, `twin_resolve`, and `why`
//! - `environment.unresolvedThirdPartyModules` (sorted, capped at 10, warm-preserved)
//! - `syncCallerCount` / `sampleSyncCaller` rendered in grouped text and `why` text

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn bin() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_async-doctor"));
    cmd.env("NO_COLOR", "1");
    cmd.env_remove("VIRTUAL_ENV");
    cmd.env_remove("CONDA_PREFIX");
    cmd.env("PATH", "/nonexistent-bin");
    cmd
}

fn shared_advice_venv() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/advice_shared_venv")
}

#[test]
fn cli_python_flag_resolves_external_venv_and_twins_cold_and_warm() {
    let tmp = tempfile::tempdir().unwrap();
    let proj = tmp.path().join("proj");
    let cache = tmp.path().join("cache");
    fs::create_dir_all(&proj).unwrap();
    fs::create_dir_all(&cache).unwrap();

    fs::write(
        proj.join("app.py"),
        "import asyncio\nimport httpx\nimport requests\n\n\
         async def fetch():\n    \
         requests.get('https://example.com')\n    \
         await asyncio.to_thread(httpx.get, 'https://example.com')\n",
    )
    .unwrap();

    // 1. Without --python: neither `requests` nor `httpx` resolves.
    let no_py = bin()
        .env("XDG_CACHE_HOME", &cache)
        .args(["--format", "json"])
        .arg(&proj)
        .output()
        .expect("run without --python");
    assert_eq!(no_py.status.code(), Some(0), "{:?}", no_py);
    let v_no_py: serde_json::Value = serde_json::from_slice(&no_py.stdout).unwrap();
    assert_eq!(v_no_py["findings"].as_array().unwrap().len(), 0);
    assert_eq!(v_no_py["environment"]["unresolvedThirdParty"], 2);
    assert_eq!(
        v_no_py["environment"]["unresolvedThirdPartyModules"],
        serde_json::json!(["httpx", "requests"])
    );

    // 2. With --python <venv> against the SAME cache dir: must miss the no-python
    // cache entry, resolve `requests.get` as `blocking-http`, and resolve
    // `httpx.AsyncClient` for `offload-has-async-twin` advice.
    let venv = shared_advice_venv();
    let cold = bin()
        .env("XDG_CACHE_HOME", &cache)
        .args(["--format", "json", "--python"])
        .arg(&venv)
        .arg(&proj)
        .output()
        .expect("run cold with --python");
    assert_eq!(
        cold.status.code(),
        Some(1),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&cold.stdout),
        String::from_utf8_lossy(&cold.stderr)
    );
    let v_cold: serde_json::Value = serde_json::from_slice(&cold.stdout).unwrap();
    assert_eq!(v_cold["environment"]["origin"], "--python");
    assert_eq!(v_cold["environment"]["unresolvedThirdParty"], 0);
    assert!(
        v_cold["environment"]
            .get("unresolvedThirdPartyModules")
            .is_none(),
        "empty unresolvedThirdPartyModules must be omitted: {}",
        v_cold["environment"]
    );
    assert_eq!(v_cold["findings"].as_array().unwrap().len(), 1);
    assert_eq!(v_cold["findings"][0]["rule"], "blocking-http");
    let why_cmd = v_cold["findings"][0]["why"].as_str().unwrap();
    assert!(
        why_cmd.contains("--python "),
        "why command should preserve --python: {why_cmd}"
    );
    let has_twin_advice = v_cold["advice"]
        .as_array()
        .unwrap()
        .iter()
        .any(|a| a["rule"] == "offload-has-async-twin");
    assert!(
        has_twin_advice,
        "twin_resolve must inspect --python site-packages: {:?}",
        v_cold["advice"]
    );

    // 3. Warm scan with --python must match cold JSON.
    let warm = bin()
        .env("XDG_CACHE_HOME", &cache)
        .args(["--format", "json", "--python"])
        .arg(&venv)
        .arg(&proj)
        .output()
        .expect("run warm with --python");
    assert_eq!(warm.status.code(), Some(1));
    let v_warm: serde_json::Value = serde_json::from_slice(&warm.stdout).unwrap();
    assert_eq!(v_cold, v_warm);

    // 4. `why` subcommand with `--python` locates the `blocking-http` site.
    let why_out = bin()
        .env("XDG_CACHE_HOME", &cache)
        .args(["why", "app.py:6", "--path"])
        .arg(&proj)
        .arg("--python")
        .arg(&venv)
        .output()
        .expect("run why with --python");
    assert_eq!(
        why_out.status.code(),
        Some(1),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&why_out.stdout),
        String::from_utf8_lossy(&why_out.stderr)
    );
    let why_stdout = String::from_utf8_lossy(&why_out.stdout);
    assert!(why_stdout.contains("blocking-http"), "{why_stdout}");

    // 5. Nonexistent `--python` exits with tool error (2).
    let bad = bin()
        .env("XDG_CACHE_HOME", &cache)
        .args(["--python", "/nonexistent/python-env-xyz"])
        .arg(&proj)
        .output()
        .expect("run invalid --python");
    assert_eq!(bad.status.code(), Some(2));
}

#[test]
fn cli_unresolved_third_party_modules_sorted_and_capped_at_ten() {
    let tmp = tempfile::tempdir().unwrap();
    let proj = tmp.path().join("proj");
    let cache = tmp.path().join("cache");
    fs::create_dir_all(&proj).unwrap();
    fs::create_dir_all(&cache).unwrap();

    fs::write(
        proj.join("app.py"),
        "import z_pkg\nimport b_pkg\nimport a_pkg\nimport d_pkg\nimport c_pkg\n\
         import f_pkg\nimport e_pkg\nimport h_pkg\nimport g_pkg\nimport j_pkg\n\
         import i_pkg\nimport k_pkg\n\n\
         async def handler():\n    return 1\n",
    )
    .unwrap();

    let run = |cache_dir: &Path| -> serde_json::Value {
        let out = bin()
            .env("XDG_CACHE_HOME", cache_dir)
            .args(["--format", "json"])
            .arg(&proj)
            .output()
            .expect("scan");
        assert_eq!(out.status.code(), Some(0));
        serde_json::from_slice(&out.stdout).unwrap()
    };

    let cold = run(&cache);
    let warm = run(&cache);
    let expected = serde_json::json!([
        "a_pkg", "b_pkg", "c_pkg", "d_pkg", "e_pkg", "f_pkg", "g_pkg", "h_pkg", "i_pkg", "j_pkg"
    ]);
    assert_eq!(cold["environment"]["unresolvedThirdParty"], 12);
    assert_eq!(cold["environment"]["unresolvedThirdPartyModules"], expected);
    assert_eq!(warm["environment"]["unresolvedThirdPartyModules"], expected);
}

#[test]
fn cli_grouped_text_and_why_render_sync_caller_count_and_sample() {
    let tmp = tempfile::tempdir().unwrap();
    let proj = tmp.path().join("proj");
    let cache = tmp.path().join("cache");
    fs::create_dir_all(&proj).unwrap();
    fs::create_dir_all(&cache).unwrap();

    fs::write(
        proj.join("helpers.py"),
        "import time\n\ndef load_data():\n    time.sleep(1)\n",
    )
    .unwrap();
    fs::write(
        proj.join("routes.py"),
        "from helpers import load_data\n\n\
         async def first_route():\n    load_data()\n\n\
         async def second_route():\n    load_data()\n",
    )
    .unwrap();
    fs::write(
        proj.join("cli_job.py"),
        "from helpers import load_data\n\ndef run_cli():\n    load_data()\n",
    )
    .unwrap();

    let grouped = bin()
        .env("XDG_CACHE_HOME", &cache)
        .arg(&proj)
        .output()
        .expect("grouped text");
    assert_eq!(grouped.status.code(), Some(1));
    let grouped_stdout = String::from_utf8_lossy(&grouped.stdout);
    assert!(
        grouped_stdout.contains("1 sync caller") && grouped_stdout.contains("cli_job.py:4"),
        "grouped text must surface sync_caller_count and sample_sync_caller:\n{grouped_stdout}"
    );

    let why = bin()
        .env("XDG_CACHE_HOME", &cache)
        .args(["why", "helpers.py:4", "--path"])
        .arg(&proj)
        .output()
        .expect("why text");
    assert_eq!(why.status.code(), Some(1));
    let why_stdout = String::from_utf8_lossy(&why.stdout);
    assert!(
        why_stdout.contains("1 sync caller also reaches this helper")
            && why_stdout.contains("cli_job.py:4"),
        "why text output must surface sync caller info with singular verb:\n{why_stdout}"
    );
}

fn git(dir: &Path, args: &[&str]) {
    let st = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "async-doctor")
        .env("GIT_AUTHOR_EMAIL", "async-doctor@users.noreply.github.com")
        .env("GIT_COMMITTER_NAME", "async-doctor")
        .env(
            "GIT_COMMITTER_EMAIL",
            "async-doctor@users.noreply.github.com",
        )
        .status()
        .expect("git");
    assert!(st.success(), "git {args:?} failed");
}

#[test]
fn cli_changed_retains_cross_file_def_to_async_def_for_all_four_rules_cold_and_warm() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    let cache = tmp.path().join("cache");
    fs::create_dir_all(&repo).unwrap();
    fs::create_dir_all(&cache).unwrap();

    git(&repo, &["init", "-b", "main"]);
    fs::write(
        repo.join("lib.py"),
        "def notify() -> int:\n    return 1\n\n\
         def offload_target() -> int:\n    return 2\n\n\
         class Managed:\n    \
             def __enter__(self):\n        return self\n    \
             def __exit__(self, *args):\n        return False\n\n\
         def stream_items():\n    yield 1\n",
    )
    .unwrap();
    fs::write(
        repo.join("app.py"),
        "import asyncio\n\
         from lib import Managed, notify, offload_target, stream_items\n\n\
         async def route_unawaited():\n    notify()\n\n\
         async def route_offload():\n    await asyncio.to_thread(offload_target)\n\n\
         async def route_with():\n    with Managed():\n        pass\n\n\
         async def route_for():\n    for _ in stream_items():\n        pass\n",
    )
    .unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-m", "sync baseline"]);

    // Modify ONLY `lib.py` (`def` -> `async def`, `__enter__` -> `__aenter__`).
    // `app.py` is 100% untouched in git.
    fs::write(
        repo.join("lib.py"),
        "async def notify() -> int:\n    return 1\n\n\
         async def offload_target() -> int:\n    return 2\n\n\
         class Managed:\n    \
             async def __aenter__(self):\n        return self\n    \
             async def __aexit__(self, *args):\n        return False\n\n\
         async def stream_items():\n    yield 1\n",
    )
    .unwrap();

    for pass in ["cold", "warm"] {
        let out = Command::new(env!("CARGO_BIN_EXE_async-doctor"))
            .env("NO_COLOR", "1")
            .env_remove("VIRTUAL_ENV")
            .env_remove("CONDA_PREFIX")
            .env("XDG_CACHE_HOME", &cache)
            .args(["--format", "json", "--changed=HEAD"])
            .arg(&repo)
            .output()
            .expect("run --changed");
        assert_eq!(
            out.status.code(),
            Some(1),
            "{pass}: expected exit 1, stdout={} stderr={}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        let rules: std::collections::BTreeSet<&str> = v["findings"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| f["rule"].as_str().unwrap())
            .collect();
        assert_eq!(
            rules,
            std::collections::BTreeSet::from([
                "offload-coroutine",
                "sync-for-async-iterable",
                "sync-with-async-cm",
                "unawaited-coroutine",
            ]),
            "{pass}: findings={}",
            v["findings"]
        );
    }
}

#[test]
fn cli_changed_retains_intra_function_guard_removal_and_class_init_edit() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    let cache = tmp.path().join("cache");
    fs::create_dir_all(&repo).unwrap();
    fs::create_dir_all(&cache).unwrap();

    git(&repo, &["init", "-b", "main"]);
    fs::write(
        repo.join("service.py"),
        "import time\nimport urllib.request\n\n\
         def guarded_helper() -> int:\n    \
             if False:\n        \
                 return 0\n    \
             else:\n        \
                 return 1\n    \
             x = 1\n    \
             y = 2\n    \
             time.sleep(1)\n    \
             return x + y\n\n\
         class SyncClient:\n    \
             def get(self) -> bytes:\n        \
                 return urllib.request.urlopen('https://example.com').read()\n\n\
         class AsyncSafeClient:\n    \
             def get(self) -> bytes:\n        \
                 return b'ok'\n\n\
         class Worker:\n    \
             def __init__(self) -> None:\n        \
                 self.client = AsyncSafeClient()\n\n    \
             async def handle(self) -> bytes:\n        \
                 return self.client.get()\n\n\
         async def untouched_legacy() -> None:\n    \
             time.sleep(9)\n",
    )
    .unwrap();
    fs::write(
        repo.join("routes.py"),
        "from service import guarded_helper\n\n\
         async def route() -> int:\n    return guarded_helper()\n",
    )
    .unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-m", "initial safe"]);

    // 1. Remove the early-return guard at top of `guarded_helper` (lines away from `time.sleep(1)`)
    //    AND swap `self.client = SyncClient()` inside `Worker.__init__` (`Worker.handle` untouched).
    //    `untouched_legacy` at the bottom of `service.py` must stay filtered out!
    fs::write(
        repo.join("service.py"),
        "import time\nimport urllib.request\n\n\
         def guarded_helper() -> int:\n    \
             x = 1\n    \
             y = 2\n    \
             time.sleep(1)\n    \
             return x + y\n\n\
         class SyncClient:\n    \
             def get(self) -> bytes:\n        \
                 return urllib.request.urlopen('https://example.com').read()\n\n\
         class AsyncSafeClient:\n    \
             def get(self) -> bytes:\n        \
                 return b'ok'\n\n\
         class Worker:\n    \
             def __init__(self) -> None:\n        \
                 self.client = SyncClient()\n\n    \
             async def handle(self) -> bytes:\n        \
                 return self.client.get()\n\n\
         async def untouched_legacy() -> None:\n    \
             time.sleep(9)\n",
    )
    .unwrap();

    for pass in ["cold", "warm"] {
        let out = Command::new(env!("CARGO_BIN_EXE_async-doctor"))
            .env("NO_COLOR", "1")
            .env_remove("VIRTUAL_ENV")
            .env_remove("CONDA_PREFIX")
            .env("XDG_CACHE_HOME", &cache)
            .args(["--format", "json", "--changed=HEAD"])
            .arg(&repo)
            .output()
            .expect("run --changed");
        assert_eq!(out.status.code(), Some(1), "{pass}");
        let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        let entries: std::collections::BTreeSet<&str> = v["findings"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| f["entry"]["name"].as_str().unwrap())
            .collect();
        assert_eq!(
            entries,
            std::collections::BTreeSet::from(["Worker.handle", "route"]),
            "{pass}: expected guarded_helper + Worker.__init__ findings without untouched_legacy: {}",
            v["findings"]
        );
    }
}

#[test]
fn cli_llm_advice_rules_exit_zero_and_explain_works() {
    let tmp = tempfile::tempdir().unwrap();
    let proj = tmp.path().join("proj");
    let cache = tmp.path().join("cache");
    fs::create_dir_all(&proj).unwrap();
    fs::create_dir_all(&cache).unwrap();

    fs::write(
        proj.join("app.py"),
        "import asyncio\nimport sqlite3\n\n\
         async def route_db():\n    \
             conn = sqlite3.connect(':memory:')\n    \
             return await asyncio.to_thread(conn.execute, 'SELECT 1')\n\n\
         async def route_lock():\n    \
             async with asyncio.Lock():\n        \
                 await asyncio.sleep(0)\n\n\
         async def route_cancel():\n    \
             try:\n        \
                 await asyncio.sleep(0)\n    \
             except asyncio.CancelledError:\n        \
                 return None\n",
    )
    .unwrap();

    let out = bin()
        .env("XDG_CACHE_HOME", &cache)
        .args(["--format", "json"])
        .arg(&proj)
        .output()
        .expect("run json");
    assert_eq!(
        out.status.code(),
        Some(0),
        "advice-only report must exit 0: {}",
        String::from_utf8_lossy(&out.stdout)
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(v["findings"].as_array().unwrap().is_empty());
    let advice_rules: std::collections::BTreeSet<&str> = v["advice"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["rule"].as_str().unwrap())
        .collect();
    assert_eq!(
        advice_rules,
        std::collections::BTreeSet::from([
            "offload-stateful-session",
            "swallowed-cancelled-error",
            "useless-local-async-lock",
        ]),
        "advice={}",
        v["advice"]
    );

    for rule in [
        "offload-stateful-session",
        "useless-local-async-lock",
        "swallowed-cancelled-error",
    ] {
        let ex = bin().args(["explain", rule]).output().expect("explain");
        assert_eq!(ex.status.code(), Some(0), "explain {rule}");
        let stdout = String::from_utf8_lossy(&ex.stdout);
        assert!(stdout.contains(rule), "{stdout}");
    }
}
