//! Stable JSON contract (schemaVersion 2): always-emitted keys + environment.

use std::path::PathBuf;
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

/// Top-level keys that must always appear on `--format json` output.
const REQUIRED_KEYS: &[&str] = &[
    "schemaVersion",
    "complete",
    "score",
    "environment",
    "findings",
    "candidates",
    "advice",
    "locationLessCandidates",
    "skipped",
    "hiddenUnresolvedCandidates",
    "omittedNonProductionEntries",
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
        .env_remove("VIRTUAL_ENV")
        .env_remove("CONDA_PREFIX")
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
    assert_eq!(v["hiddenUnresolvedCandidates"], 0);
    assert_eq!(v["omittedNonProductionEntries"], 0);
    assert!(v["environment"].is_object());
    assert!(v["environment"]["kind"].is_string());
    assert!(v["environment"]["sitePackages"].is_array());
    // Stdlib-only + no project venv → no advisory note (omitted or empty).
    let note = v["environment"]["note"].as_str().unwrap_or("");
    assert!(
        note.is_empty(),
        "stdlib-only must not warn about VIRTUAL_ENV: {}",
        v["environment"]
    );
}

#[test]
fn environment_note_when_no_venv_and_missing_third_party() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(
        tmp.path().join("app.py"),
        "import definitely_missing_pkg_xyz\n\nasync def noop():\n    return 1\n",
    )
    .unwrap();

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
    assert!(
        kind == "system" || kind == "none",
        "expected system/none without project venv, got {env}"
    );
    assert!(
        env["unresolvedThirdParty"].as_u64().unwrap_or(0) >= 1,
        "expected unresolved third-party count: {env}"
    );
    let note = env["note"].as_str().unwrap_or("");
    assert!(
        note.contains("VIRTUAL_ENV") || note.contains("venv"),
        "expected venv advisory note, got {env}"
    );
    assert!(
        !v["locationLessCandidates"].as_array().unwrap().is_empty(),
        "missing third-party aggregate must be location-less: {}",
        v["locationLessCandidates"]
    );
    let why = v["locationLessCandidates"][0]["why"]
        .as_str()
        .unwrap_or("x");
    assert!(
        why.is_empty(),
        "location-less why must be empty (not a fake command): {why}"
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
    // Do not double-print the unresolved-import aggregate alongside the note.
    assert!(
        !stdout.contains("[unresolved-import]"),
        "text must not duplicate the note as a location-less finding: {stdout}"
    );
}

#[test]
fn why_path_is_relative_not_absolute() {
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
        path_arg.contains("m1_multihop"),
        "expected relative fixtures/m1_multihop path, got {why}"
    );
}
