//! CLI integration: `explain` / `why`, chain + docsAnchor in JSON, exit codes.

use std::path::PathBuf;
use std::process::Command;

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_async-doctor"))
}

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("fixtures")
        .join(name)
        .canonicalize()
        .expect("fixture path")
}

#[test]
fn explain_blocking_sleep_prints_fix_and_anchor() {
    let out = bin()
        .args(["explain", "blocking-sleep"])
        .output()
        .expect("run");
    assert!(
        out.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("blocking-sleep"));
    assert!(stdout.contains("asyncio.sleep"));
    assert!(stdout.contains("#blocking-sleep"));
    assert!(stdout.contains("docs/rules.md"));
}

#[test]
fn explain_unknown_rule_exits_2() {
    let out = bin()
        .args(["explain", "not-a-real-rule"])
        .output()
        .expect("run");
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("unknown rule"), "{stderr}");
}

#[test]
fn explain_accepts_hash_anchor() {
    let out = bin()
        .args(["explain", "#blocking-http"])
        .output()
        .expect("run");
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("blocking-http"));
    assert!(stdout.contains("AsyncClient") || stdout.contains("aiohttp"));
}

#[test]
fn json_includes_docs_anchor_and_full_chain() {
    let dir = fixture("m1_multihop");
    let out = bin()
        .args(["--format", "json"])
        .arg(&dir)
        .output()
        .expect("run");
    assert_eq!(
        out.status.code(),
        Some(1),
        "stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("json");
    assert_eq!(v["schemaVersion"], 1);
    let finding = &v["findings"][0];
    assert_eq!(finding["rule"], "blocking-sleep");
    assert_eq!(finding["docsAnchor"], "#blocking-sleep");
    let chain = finding["chain"].as_array().expect("chain");
    assert!(chain.len() >= 2, "expected multi-hop chain, got {chain:?}");
    assert!(
        chain
            .iter()
            .all(|s| s["file"].as_str().is_some() && s["line"].as_u64().is_some())
    );
    let why = finding["why"].as_str().expect("why");
    assert!(why.starts_with("async-doctor why "), "{why}");
    let fix = finding["fix"]["hint"].as_str().expect("fix");
    assert!(
        fix.contains("asyncio.sleep") || fix.contains("to_thread"),
        "{fix}"
    );
}

#[test]
fn text_output_shows_chain_fix_explain_why() {
    let dir = fixture("m1_multihop");
    let out = bin().arg(&dir).output().expect("run");
    assert_eq!(out.status.code(), Some(1));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("[blocking-sleep]"));
    assert!(stdout.contains("├─") || stdout.contains("└─"));
    assert!(stdout.contains("fix:"));
    assert!(stdout.contains("explain: async-doctor explain blocking-sleep"));
    assert!(stdout.contains("why: async-doctor why "));
}

#[test]
fn why_locates_multihop_sink() {
    let dir = fixture("m1_multihop");
    // Discover the sink location from JSON first.
    let json = bin()
        .args(["--format", "json"])
        .arg(&dir)
        .output()
        .expect("run");
    let v: serde_json::Value = serde_json::from_slice(&json.stdout).unwrap();
    let why = v["findings"][0]["why"].as_str().unwrap();
    let location = why.strip_prefix("async-doctor why ").unwrap();

    let out = bin()
        .args(["why", location, "--path"])
        .arg(&dir)
        .output()
        .expect("run");
    assert_eq!(
        out.status.code(),
        Some(1),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("Evidence for "));
    assert!(stdout.contains("blocking-sleep"));
    assert!(stdout.contains("Why it matters") || stdout.contains("asyncio.sleep"));
}

#[test]
fn why_miss_exits_0() {
    let dir = fixture("m1_multihop");
    let out = bin()
        .args(["why", "nope.py:999", "--path"])
        .arg(&dir)
        .output()
        .expect("run");
    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("No findings"));
}

#[test]
fn clean_fixture_exits_0() {
    let file = fixture("negative_sync.py");
    let out = bin().arg(&file).output().expect("run");
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn invalid_path_exits_2() {
    let out = bin()
        .arg("/nonexistent/async-doctor-path-xyz")
        .output()
        .expect("run");
    assert_eq!(out.status.code(), Some(2));
}
