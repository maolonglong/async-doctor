//! CLI integration: --changed, --baseline, exit codes, inline suppressions.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use tempfile::TempDir;

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_async-doctor"))
}

fn workspace_fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("fixtures")
        .join(name)
        .canonicalize()
        .expect("fixture")
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

fn init_repo_with_sleep() -> TempDir {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    git(root, &["init", "-b", "main"]);
    git(root, &["config", "user.email", "async-doctor@example.com"]);
    git(root, &["config", "user.name", "async-doctor"]);
    fs::write(
        root.join("app.py"),
        "import time\n\nasync def handler():\n    time.sleep(1)\n",
    )
    .unwrap();
    fs::write(root.join("clean.py"), "async def ok():\n    return 1\n").unwrap();
    git(root, &["add", "."]);
    git(root, &["commit", "-m", "init"]);
    tmp
}

#[test]
fn write_baseline_exits_0() {
    let dir = workspace_fixture("positive_sleep.py");
    let tmp = TempDir::new().unwrap();
    let baseline = tmp.path().join("baseline.json");

    let write = bin()
        .args(["--format", "json", "--write-baseline"])
        .arg(&baseline)
        .arg(&dir)
        .output()
        .unwrap();
    assert_eq!(
        write.status.code(),
        Some(0),
        "stderr={}",
        String::from_utf8_lossy(&write.stderr)
    );
    assert!(baseline.is_file());
}

#[test]
fn baseline_suppresses_known_and_reports_count() {
    let dir = workspace_fixture("positive_sleep.py");
    let tmp = TempDir::new().unwrap();
    let baseline = tmp.path().join("baseline.json");

    assert_eq!(
        bin()
            .args(["--write-baseline"])
            .arg(&baseline)
            .arg(&dir)
            .output()
            .unwrap()
            .status
            .code(),
        Some(0)
    );

    let again = bin()
        .args(["--baseline"])
        .arg(&baseline)
        .arg(&dir)
        .output()
        .unwrap();
    assert_eq!(again.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&again.stdout);
    assert!(stdout.contains("suppressed by baseline"), "{stdout}");
}

#[test]
fn baseline_stable_across_line_shift_and_unrelated_rename() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    let src = "import time\n\nasync def handler():\n    time.sleep(1)\n";
    fs::write(root.join("app.py"), src).unwrap();

    let baseline = root.join("baseline.json");
    let first = bin()
        .args(["--format", "json", "--write-baseline"])
        .arg(&baseline)
        .arg(root)
        .output()
        .unwrap();
    assert_eq!(first.status.code(), Some(0));
    let v1: serde_json::Value = serde_json::from_slice(&first.stdout).unwrap();
    let id = v1["findings"][0]["id"].as_str().unwrap().to_string();

    let shifted = "import time\n\n# padding\n# more\n\nasync def handler():\n    time.sleep(1)\n\ndef unrelated_renamed():\n    return 0\n";
    fs::write(root.join("app.py"), shifted).unwrap();

    let second = bin()
        .args(["--format", "json", "--baseline"])
        .arg(&baseline)
        .arg(root)
        .output()
        .unwrap();
    assert_eq!(second.status.code(), Some(0));
    let v2: serde_json::Value = serde_json::from_slice(&second.stdout).unwrap();
    assert!(v2["findings"].as_array().unwrap().is_empty());

    let raw = bin().args(["--format", "json"]).arg(root).output().unwrap();
    let v3: serde_json::Value = serde_json::from_slice(&raw.stdout).unwrap();
    assert_eq!(v3["findings"][0]["id"], id);
}

#[test]
fn baseline_reports_new_finding_exit_1() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    fs::write(
        root.join("app.py"),
        "import time\n\nasync def handler():\n    time.sleep(1)\n",
    )
    .unwrap();
    let baseline = root.join("baseline.json");
    assert_eq!(
        bin()
            .args(["--write-baseline"])
            .arg(&baseline)
            .arg(root)
            .output()
            .unwrap()
            .status
            .code(),
        Some(0)
    );

    fs::write(
        root.join("app.py"),
        "import time\n\nasync def handler():\n    time.sleep(1)\n    time.sleep(2)\n",
    )
    .unwrap();
    let out = bin()
        .args(["--format", "json", "--baseline"])
        .arg(&baseline)
        .arg(root)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["findings"].as_array().unwrap().len(), 1);
}

#[test]
fn changed_filters_to_touched_files_only() {
    let repo = init_repo_with_sleep();
    let root = repo.path();

    fs::write(root.join("clean.py"), "async def ok():\n    return 2\n").unwrap();

    let out = bin()
        .args(["--format", "json", "--changed=HEAD"])
        .arg(root)
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(v["findings"].as_array().unwrap().is_empty());

    fs::write(
        root.join("app.py"),
        "import time\n\nasync def handler():\n    time.sleep(2)\n",
    )
    .unwrap();
    let out = bin()
        .args(["--format", "json", "--changed=HEAD"])
        .arg(root)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["findings"].as_array().unwrap().len(), 1);
}

#[test]
fn changed_outside_git_falls_back_to_full_scan() {
    let tmp = TempDir::new().unwrap();
    fs::write(
        tmp.path().join("app.py"),
        "import time\n\nasync def handler():\n    time.sleep(1)\n",
    )
    .unwrap();
    let out = bin()
        .args(["--format", "json", "--changed"])
        .arg(tmp.path())
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(1),
        "stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("not inside a git repository") || stderr.contains("reporting all"),
        "{stderr}"
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["findings"].as_array().unwrap().len(), 1);
}

#[test]
fn changed_monorepo_scan_subdir_ignores_sibling_change() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    git(root, &["init", "-b", "main"]);
    git(root, &["config", "user.email", "a@b.c"]);
    git(root, &["config", "user.name", "a"]);
    fs::create_dir_all(root.join("backend")).unwrap();
    fs::create_dir_all(root.join("tools")).unwrap();
    fs::write(
        root.join("backend/app.py"),
        "import time\n\nasync def handler():\n    time.sleep(1)\n",
    )
    .unwrap();
    fs::write(
        root.join("tools/helpers.py"),
        "def helper():\n    return 1\n",
    )
    .unwrap();
    git(root, &["add", "."]);
    git(root, &["commit", "-m", "init"]);

    // Change only tools/helpers.py; scan backend/ — expect 0 reported.
    fs::write(
        root.join("tools/helpers.py"),
        "def helper():\n    return 2\n",
    )
    .unwrap();
    let out = bin()
        .args(["--format", "json", "--changed=HEAD"])
        .arg(root.join("backend"))
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr={} stdout={}",
        String::from_utf8_lossy(&out.stderr),
        String::from_utf8_lossy(&out.stdout)
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(
        v["findings"].as_array().unwrap().is_empty(),
        "monorepo false match: {}",
        String::from_utf8_lossy(&out.stdout)
    );
}

#[test]
fn changed_without_ref_uses_merge_base_default() {
    let repo = init_repo_with_sleep();
    let root = repo.path();
    fs::write(
        root.join("app.py"),
        "import time\n\nasync def handler():\n    time.sleep(9)\n",
    )
    .unwrap();
    let out = bin()
        .args(["--format", "json", "--changed"])
        .arg(root)
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(1),
        "stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn write_baseline_with_changed_is_error() {
    let repo = init_repo_with_sleep();
    let out = bin()
        .args(["--write-baseline", "/tmp/async-doctor-bl.json", "--changed"])
        .arg(repo.path())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("cannot be combined"), "{stderr}");
}

#[test]
fn inline_suppress_requires_reason_and_suppresses() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    fs::write(
        root.join("app.py"),
        "import time\n\nasync def handler():\n    time.sleep(1)  # async-doctor: ignore[blocking-sleep]: intentional sync boundary\n",
    )
    .unwrap();
    let out = bin().args(["--format", "json"]).arg(root).output().unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(v["findings"].as_array().unwrap().is_empty());
}

#[test]
fn ignore_without_reason_warns_and_does_not_suppress() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    fs::write(
        root.join("app.py"),
        "import time\n\nasync def handler():\n    time.sleep(1)  # async-doctor: ignore[blocking-sleep]\n",
    )
    .unwrap();
    let out = bin().args(["--format", "json"]).arg(root).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("requires a reason"), "{stderr}");
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["findings"].as_array().unwrap().len(), 1);
}

#[test]
fn hash_inside_string_is_not_suppression() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    fs::write(
        root.join("app.py"),
        "import time\nMSG = \"# async-doctor: ignore[blocking-sleep]: fake\"\n\nasync def handler():\n    time.sleep(1)\n",
    )
    .unwrap();
    let out = bin().args(["--format", "json"]).arg(root).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["findings"].as_array().unwrap().len(), 1);
}

#[test]
fn unused_suppression_exits_1() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    fs::write(
        root.join("app.py"),
        "async def handler():\n    return 1  # async-doctor: ignore[blocking-sleep]: leftover\n",
    )
    .unwrap();
    let out = bin().args(["--format", "json"]).arg(root).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(v["findings"].as_array().unwrap().is_empty());
    assert_eq!(v["unusedSuppressions"].as_array().unwrap().len(), 1);
}

#[test]
fn tool_error_exit_2_for_bad_baseline() {
    let dir = workspace_fixture("positive_sleep.py");
    let out = bin()
        .args(["--baseline", "/nonexistent/baseline-xyz.json"])
        .arg(&dir)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn changed_rejects_basename_suffix_across_scan_root() {
    // Root pos.py changed; scan svc/ which has its own app/pos.py with sleep.
    // Exact path equality must not treat root pos.py as matching svc/app/pos.py.
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    git(root, &["init", "-b", "main"]);
    git(root, &["config", "user.email", "a@b.c"]);
    git(root, &["config", "user.name", "a"]);
    fs::create_dir_all(root.join("svc/app")).unwrap();
    fs::write(root.join("pos.py"), "def sync():\n    return 1\n").unwrap();
    fs::write(
        root.join("svc/app/pos.py"),
        "import time\n\nasync def handler():\n    time.sleep(1)\n",
    )
    .unwrap();
    git(root, &["add", "."]);
    git(root, &["commit", "-m", "init"]);

    fs::write(root.join("pos.py"), "def sync():\n    return 2\n").unwrap();
    let out = bin()
        .args(["--format", "json", "--changed=HEAD"])
        .arg(root.join("svc"))
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr={} stdout={}",
        String::from_utf8_lossy(&out.stderr),
        String::from_utf8_lossy(&out.stdout)
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(
        v["findings"].as_array().unwrap().is_empty(),
        "root pos.py must not match svc/app/pos.py: {}",
        String::from_utf8_lossy(&out.stdout)
    );
}

#[test]
fn changed_on_main_clean_reports_zero() {
    // HEAD on the default branch with a clean tree → uncommitted-only filter,
    // not a full backlog scan (--changed must stay fast on a clean main).
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    git(root, &["init", "-b", "main"]);
    git(root, &["config", "user.email", "a@b.c"]);
    git(root, &["config", "user.name", "a"]);
    fs::write(
        root.join("app.py"),
        "import time\n\nasync def handler():\n    time.sleep(1)\n",
    )
    .unwrap();
    fs::write(
        root.join("other.py"),
        "import time\n\nasync def other():\n    time.sleep(1)\n",
    )
    .unwrap();
    git(root, &["add", "."]);
    git(root, &["commit", "-m", "backlog"]);

    let out = bin()
        .args(["--format", "json", "--changed"])
        .arg(root)
        .env_remove("GITHUB_BASE_REF")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr={stderr} stdout={}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(
        !stderr.contains("reporting all findings"),
        "must not full-scan on clean main: {stderr}"
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(
        v["findings"].as_array().unwrap().is_empty(),
        "clean main must report 0 findings, got {}",
        String::from_utf8_lossy(&out.stdout)
    );
}

#[test]
fn changed_on_main_uncommitted_filters_to_that_file() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    git(root, &["init", "-b", "main"]);
    git(root, &["config", "user.email", "a@b.c"]);
    git(root, &["config", "user.name", "a"]);
    fs::write(
        root.join("app.py"),
        "import time\n\nasync def handler():\n    time.sleep(1)\n",
    )
    .unwrap();
    fs::write(
        root.join("other.py"),
        "import time\n\nasync def other():\n    time.sleep(1)\n",
    )
    .unwrap();
    git(root, &["add", "."]);
    git(root, &["commit", "-m", "backlog"]);

    // Uncommitted edit only to app.py — other.py backlog must stay filtered out.
    fs::write(
        root.join("app.py"),
        "import time\n\nasync def handler():\n    time.sleep(2)\n",
    )
    .unwrap();

    let out = bin()
        .args(["--format", "json", "--changed"])
        .arg(root)
        .env_remove("GITHUB_BASE_REF")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(1),
        "stderr={stderr} stdout={}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(
        !stderr.contains("reporting all findings"),
        "must not full-scan on dirty main: {stderr}"
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let findings = v["findings"].as_array().unwrap();
    assert_eq!(
        findings.len(),
        1,
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let file = findings[0]["entry"]["file"].as_str().unwrap();
    assert!(
        file.ends_with("app.py"),
        "expected only app.py finding, got {file}"
    );
}

#[test]
fn changed_shallow_detached_without_base_falls_back_to_full_scan() {
    // Simulate GitHub Actions' default shape: shallow clone of a feature branch,
    // detached HEAD, no origin/main. Must not silently report clean.
    let src = TempDir::new().unwrap();
    let src_root = src.path();
    git(src_root, &["init", "-b", "main"]);
    git(src_root, &["config", "user.email", "a@b.c"]);
    git(src_root, &["config", "user.name", "a"]);
    fs::write(
        src_root.join("app.py"),
        "async def handler():\n    return 1\n",
    )
    .unwrap();
    git(src_root, &["add", "."]);
    git(src_root, &["commit", "-m", "clean"]);
    git(src_root, &["checkout", "-b", "feat"]);
    fs::write(
        src_root.join("app.py"),
        "import time\n\nasync def handler():\n    time.sleep(1)\n",
    )
    .unwrap();
    git(src_root, &["add", "."]);
    git(src_root, &["commit", "-m", "add sleep"]);

    let dst = TempDir::new().unwrap();
    let dst_root = dst.path().join("clone");
    // file:// + --single-branch so depth is honored and origin/main is absent
    // (the GHA-shaped case: detached tip of feat only).
    let src_uri = format!("file://{}", src_root.canonicalize().unwrap().display());
    let st = Command::new("git")
        .args([
            "clone",
            "--depth",
            "1",
            "--single-branch",
            "--branch",
            "feat",
            &src_uri,
            dst_root.to_str().unwrap(),
        ])
        .status()
        .unwrap();
    assert!(st.success());
    git(&dst_root, &["checkout", "--detach"]);

    let out = bin()
        .args(["--format", "json", "--changed"])
        .arg(&dst_root)
        .env_remove("GITHUB_BASE_REF")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(1),
        "stderr={stderr} stdout={}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(
        stderr.contains("reporting all findings") || stderr.contains("could not resolve"),
        "expected full-scan note, got: {stderr}"
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        v["findings"].as_array().unwrap().len(),
        1,
        "shallow detached must surface the sleep finding"
    );
}
