//! Stable JSON contract (schemaVersion 2): always-emitted keys + environment.

use std::path::PathBuf;
use std::process::Command;

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_async-doctor"))
}

/// Top-level keys that must always appear on `--format json` output.
const REQUIRED_KEYS: &[&str] = &[
    "schemaVersion",
    "complete",
    "environment",
    "findings",
    "candidates",
    "advice",
    "locationLessCandidates",
    "skipped",
    "unresolvedSummary",
    "unusedSuppressions",
    "sinkSites",
];

#[test]
fn empty_project_json_emits_stable_keys() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(
        tmp.path().join("app.py"),
        "async def noop():\n    return 1\n",
    )
    .unwrap();

    let out = bin()
        .args(["--format", "json"])
        .arg(tmp.path())
        .output()
        .expect("run");
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("json");
    assert_eq!(v["schemaVersion"], 2);
    for key in REQUIRED_KEYS {
        assert!(
            v.get(key).is_some(),
            "missing top-level key `{key}` in {}",
            serde_json::to_string_pretty(&v).unwrap()
        );
    }
    for arr in [
        "findings",
        "candidates",
        "advice",
        "locationLessCandidates",
        "skipped",
        "unresolvedSummary",
        "unusedSuppressions",
        "sinkSites",
    ] {
        assert!(
            v[arr].is_array(),
            "`{arr}` must always be an array: {}",
            v[arr]
        );
    }
    assert!(v["environment"].is_object());
    assert!(v["environment"]["kind"].is_string());
    assert!(v["environment"]["sitePackages"].is_array());
}

#[test]
fn environment_note_when_no_project_venv() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(
        tmp.path().join("app.py"),
        "async def noop():\n    return 1\n",
    )
    .unwrap();

    // Ensure VIRTUAL_ENV does not point at a project venv for this temp root.
    let mut cmd = bin();
    cmd.env_remove("VIRTUAL_ENV");
    cmd.env_remove("CONDA_PREFIX");
    let out = cmd
        .args(["--format", "json"])
        .arg(tmp.path())
        .output()
        .expect("run");
    assert_eq!(out.status.code(), Some(0));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let env = &v["environment"];
    let kind = env["kind"].as_str().unwrap_or("");
    // Temp dir has no .venv — expect system or none, with an advisory note.
    assert!(
        kind == "system" || kind == "none",
        "expected system/none without project venv, got {env}"
    );
    let note = env["note"].as_str().unwrap_or("");
    assert!(
        note.contains("VIRTUAL_ENV") || note.contains("venv"),
        "expected venv advisory note, got {env}"
    );

    let text = bin()
        .env_remove("VIRTUAL_ENV")
        .env_remove("CONDA_PREFIX")
        .arg(tmp.path())
        .output()
        .expect("run");
    let stdout = String::from_utf8_lossy(&text.stdout);
    assert!(
        stdout.contains("Environment:"),
        "text must report environment: {stdout}"
    );
    assert!(
        stdout.contains("note:") && stdout.contains("VIRTUAL_ENV"),
        "text must print venv note: {stdout}"
    );
}

#[test]
fn why_path_is_relative_not_absolute() {
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("fixtures")
        .join("m1_multihop");
    let out = bin()
        .current_dir(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.."))
        .args(["--format", "json", "fixtures/m1_multihop"])
        .output()
        .expect("run");
    assert_eq!(
        out.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let why = v["findings"][0]["why"].as_str().unwrap();
    let path_arg = why.split(" --path ").nth(1).unwrap_or("");
    assert!(
        !path_arg.starts_with('/'),
        "absolute machine path in why: {why}"
    );
    assert!(
        path_arg.contains("m1_multihop") || path_arg == "fixtures/m1_multihop",
        "{why}"
    );
    let _ = fixture;
}
