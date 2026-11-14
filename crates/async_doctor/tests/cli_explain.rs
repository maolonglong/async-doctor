//! CLI integration: `explain` / `why`, chain + docsAnchor in JSON, exit codes.

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

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("fixtures")
        .join(name)
        .canonicalize()
        .expect("fixture path")
}

#[test]
fn explain_blocking_sleep_describes_problem_without_prescribing_a_repair() {
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
    assert!(stdout.contains("time.sleep"));
    assert!(!stdout.contains("How to fix"), "{stdout}");
    assert!(!stdout.contains("to_thread"), "{stdout}");
    assert!(stdout.contains("#blocking-sleep"));
    assert!(stdout.contains(
        "https://github.com/maolonglong/async-doctor/blob/main/docs/rules.md#blocking-sleep"
    ));
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
    assert!(stdout.contains("HTTP"));
    assert!(!stdout.contains("How to fix"), "{stdout}");
}

#[test]
fn json_includes_docs_url_chain_resolution_and_runnable_why() {
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
    assert_eq!(v["schemaVersion"], 2);
    assert!(v.get("environment").is_some(), "missing environment: {v}");
    assert!(
        v["advice"].is_array()
            && v["skipped"].is_array()
            && v["sinkSites"].is_array()
            && v["locationLessCandidates"].is_array()
            && v["unusedSuppressions"].is_array()
            && v["unresolvedSummary"].is_array()
            && v.get("hiddenUnresolvedCandidates").is_some()
            && v.get("omittedNonProductionEntries").is_some(),
        "stable top-level arrays/counts required: {v}"
    );
    let finding = &v["findings"][0];
    assert_eq!(finding["rule"], "blocking-sleep");
    assert_eq!(finding["fix"]["hint"], "");
    assert!(
        finding["fix"]["fixGroupId"]
            .as_str()
            .is_some_and(|s| !s.is_empty())
    );
    assert_eq!(finding["docsAnchor"], "#blocking-sleep");
    assert!(
        finding["docsUrl"]
            .as_str()
            .unwrap()
            .starts_with("https://github.com/maolonglong/async-doctor/blob/main/docs/rules.md#"),
        "{}",
        finding["docsUrl"]
    );
    let chain = finding["chain"].as_array().expect("chain");
    assert!(chain.len() >= 2, "expected multi-hop chain, got {chain:?}");
    assert!(
        chain.iter().any(|s| s["resolution"]
            .as_str()
            .map(|r| !r.is_empty())
            .unwrap_or(false)),
        "expected resolution on at least one hop: {chain:?}"
    );
    let why = finding["why"].as_str().expect("why");
    assert!(why.starts_with("async-doctor why "), "{why}");
    assert!(why.contains(" --path "), "why must include --path: {why}");
    let path_arg = why.split(" --path ").nth(1).unwrap_or("");
    assert!(
        !path_arg.is_empty() && !path_arg.starts_with('/'),
        "why must use a relative --path, got {why}"
    );
    // Plain async def entries serialize as async_def, never opaque "other".
    assert_ne!(finding["entry"]["label"], "other");
    let label = finding["entry"]["label"].as_str().unwrap_or("");
    assert!(
        label == "async_def" || label == "route" || label == "lifecycle" || label == "task",
        "unexpected label {label}"
    );
}

#[test]
fn text_default_is_grouped_expand_lists_all() {
    let dir = fixture("m1_multihop");
    let grouped = bin().arg(&dir).output().expect("run");
    assert_eq!(grouped.status.code(), Some(1));
    let stdout = String::from_utf8_lossy(&grouped.stdout);
    assert!(
        stdout.contains("Environment:"),
        "must report interpreter/venv: {stdout}"
    );
    assert!(
        (stdout.contains("sink site") || stdout.contains("sink sites"))
            && (stdout.contains("finding") || stdout.contains("findings")),
        "summary must lead with sink sites then findings: {stdout}"
    );
    // Sink sites come before findings in the lead line.
    let lead = stdout.lines().find(|l| l.starts_with("Found ")).unwrap();
    let sink_pos = lead.find("sink site").unwrap();
    let finding_pos = lead.find("finding").unwrap();
    assert!(
        sink_pos < finding_pos,
        "sink sites must precede findings: {lead}"
    );
    assert!(
        stdout.contains("diagnostic group") || stdout.contains("sink site"),
        "{stdout}"
    );
    assert!(stdout.contains("async def handler"));
    assert!(!stdout.contains("fix:"), "{stdout}");
    assert!(!stdout.contains("Top fix:"), "{stdout}");
    assert!(!stdout.contains("clears "), "{stdout}");
    assert!(stdout.contains("top sinks"), "{stdout}");
    assert!(stdout.contains("https://github.com/maolonglong/async-doctor"));
    assert!(
        !stdout.contains("other  handler"),
        "entry kind 'other' must not print: {stdout}"
    );

    let expanded = bin().args(["--expand"]).arg(&dir).output().expect("run");
    let exp = String::from_utf8_lossy(&expanded.stdout);
    assert!(exp.contains("why: async-doctor why "));
    assert!(exp.contains(" --path "));
    assert!(exp.contains("explain: async-doctor explain blocking-sleep"));
    assert!(!exp.contains("fix:"), "{exp}");
}

#[test]
fn why_locates_multihop_sink_with_resolution() {
    let dir = fixture("m1_multihop");
    let json = bin()
        .args(["--format", "json"])
        .arg(&dir)
        .output()
        .expect("run");
    let v: serde_json::Value = serde_json::from_slice(&json.stdout).unwrap();
    let why = v["findings"][0]["why"].as_str().unwrap();
    // `async-doctor why helpers.py:5 --path /abs`
    let rest = why.strip_prefix("async-doctor why ").unwrap();
    let location = rest.split_whitespace().next().unwrap();

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
    assert!(stdout.contains("Hop resolution:"));
    assert!(
        stdout.contains("blocking sink") || stdout.contains("resolved call"),
        "{stdout}"
    );
    assert!(!stdout.contains("How to fix"), "{stdout}");

    let json = bin()
        .args(["why", location, "--format", "json", "--path"])
        .arg(&dir)
        .output()
        .expect("run why json");
    assert_eq!(json.status.code(), Some(1));
    let why_json: serde_json::Value = serde_json::from_slice(&json.stdout).unwrap();
    assert_eq!(why_json["findings"][0]["fix"]["hint"], "");
    assert_eq!(why_json["rules"][0]["fix"], "");
}

#[test]
fn why_path_components_not_string_suffix() {
    // Create a tiny project where `a.py:3` must not match a finding in `data.py`.
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(
        tmp.path().join("data.py"),
        "import time\n\nasync def handler():\n    time.sleep(1)\n",
    )
    .unwrap();
    std::fs::write(
        tmp.path().join("a.py"),
        "async def other():\n    return 1\n",
    )
    .unwrap();

    let out = bin()
        .args(["why", "a.py:3", "--path"])
        .arg(tmp.path())
        .output()
        .expect("run");
    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("No findings"), "{stdout}");
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

#[test]
fn protocol_corrections_do_not_require_converting_a_sync_api() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("app.py"),
        r#"class OnlyAsyncCM:
    async def __aenter__(self): return self
    async def __aexit__(self, *args): return False

async def rows():
    yield 7

def sync_api():
    with OnlyAsyncCM():
        pass
    for row in rows():
        pass

async def direct():
    with OnlyAsyncCM():
        pass
    for row in rows():
        pass

async def via_sync_api():
    sync_api()
"#,
    )
    .unwrap();
    let out = bin()
        .args(["--format", "json"])
        .arg(dir.path())
        .output()
        .expect("run protocol boundaries");
    assert_eq!(out.status.code(), Some(1));
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["complete"], true);
    let findings = report["findings"].as_array().unwrap();
    assert_eq!(findings.len(), 4, "{report}");
    for finding in findings {
        let hint = finding["fix"]["hint"].as_str().unwrap();
        match finding["entry"]["name"].as_str().unwrap() {
            "direct" => assert!(!hint.is_empty(), "{finding}"),
            "via_sync_api" => assert!(hint.is_empty(), "{finding}"),
            entry => panic!("unexpected entry {entry}: {finding}"),
        }
    }

    for (location, has_correction) in [
        ("app.py:9", false),
        ("app.py:11", false),
        ("app.py:15", true),
        ("app.py:17", true),
    ] {
        let out = bin()
            .args(["why", location, "--format", "json", "--path"])
            .arg(dir.path())
            .output()
            .expect("run contextual why json");
        assert_eq!(out.status.code(), Some(1), "{location}");
        let why: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(why["findings"].as_array().unwrap().len(), 1, "{why}");
        assert_eq!(why["rules"].as_array().unwrap().len(), 1, "{why}");
        assert_eq!(
            !why["rules"][0]["fix"].as_str().unwrap().is_empty(),
            has_correction,
            "{location}: {why}"
        );

        let out = bin()
            .args(["why", location, "--path"])
            .arg(dir.path())
            .output()
            .expect("run contextual why text");
        assert_eq!(out.status.code(), Some(1), "{location}");
        let text = String::from_utf8(out.stdout).unwrap();
        assert!(text.contains("Hop resolution:"), "{text}");
        assert_eq!(text.contains("How to fix"), has_correction, "{text}");
    }
}
