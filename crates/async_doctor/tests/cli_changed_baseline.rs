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
        .env("GIT_AUTHOR_EMAIL", "async-doctor@example.com")
        .env("GIT_COMMITTER_NAME", "async-doctor")
        .env("GIT_COMMITTER_EMAIL", "async-doctor@example.com")
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
fn baseline_suppresses_known_and_exits_0() {
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
        Some(1),
        "{}",
        String::from_utf8_lossy(&write.stderr)
    );
    assert!(baseline.is_file());

    let again = bin()
        .args(["--format", "json", "--baseline"])
        .arg(&baseline)
        .arg(&dir)
        .output()
        .unwrap();
    assert_eq!(
        again.status.code(),
        Some(0),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&again.stdout),
        String::from_utf8_lossy(&again.stderr)
    );
    let v: serde_json::Value = serde_json::from_slice(&again.stdout).unwrap();
    assert!(v["findings"].as_array().unwrap().is_empty());
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
    assert_eq!(first.status.code(), Some(1));
    let v1: serde_json::Value = serde_json::from_slice(&first.stdout).unwrap();
    let id = v1["findings"][0]["id"].as_str().unwrap().to_string();

    // Shift lines + rename unrelated symbol; sink/entry qualnames unchanged.
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

    // Without baseline, same id after the shift.
    let raw = bin().args(["--format", "json"]).arg(root).output().unwrap();
    let v3: serde_json::Value = serde_json::from_slice(&raw.stdout).unwrap();
    assert_eq!(v3["findings"][0]["id"], id);
    assert_ne!(
        v3["findings"][0]["entry"]["line"],
        v1["findings"][0]["entry"]["line"]
    );
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
        Some(1)
    );

    // Add a second distinct sleep call → new occurrence id.
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

    // Modify only clean.py (no findings) — --changed should report nothing
    // even though app.py still has a sleep in the full analysis.
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

    // Touch app.py → finding reported.
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
fn changed_without_ref_uses_merge_base_default() {
    let repo = init_repo_with_sleep();
    let root = repo.path();
    // Uncommitted edit to app.py.
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
    // In a single-branch repo, merge-base with main is HEAD; uncommitted
    // changes still count, so app.py is "changed".
    assert_eq!(
        out.status.code(),
        Some(1),
        "stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
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
    assert_eq!(v["unusedSuppressions"][0]["rule"], "blocking-sleep");
}

#[test]
fn ignore_without_reason_does_not_suppress() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    fs::write(
        root.join("app.py"),
        "import time\n\nasync def handler():\n    time.sleep(1)  # async-doctor: ignore[blocking-sleep]\n",
    )
    .unwrap();
    let out = bin().args(["--format", "json"]).arg(root).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["findings"].as_array().unwrap().len(), 1);
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
