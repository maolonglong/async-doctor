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
