//! Milestone 3 CLI & AX regressions: suppressions, single-file scope,
//! `--changed`/`--diff`, `--fail-on`, `why --format json`, `--max-findings`
//! sink-site summaries, and SARIF global descriptors / URI encoding.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

use jsonschema::Draft;
use serde_json::Value;
use tempfile::TempDir;

static CACHE_COUNTER: AtomicU64 = AtomicU64::new(0);

fn bin() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_async-doctor"));
    cmd.env("NO_COLOR", "1");
    cmd.env_remove("VIRTUAL_ENV").env_remove("CONDA_PREFIX");
    let seq = CACHE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let cache = std::env::temp_dir().join(format!(
        "async-doctor-m3-cache-{}-{seq}",
        std::process::id(),
    ));
    let _ = fs::create_dir_all(&cache);
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

fn sarif_validator() -> &'static jsonschema::Validator {
    static V: OnceLock<jsonschema::Validator> = OnceLock::new();
    V.get_or_init(|| {
        let path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/data/sarif-schema-2.1.0.json");
        let schema: Value =
            serde_json::from_str(&fs::read_to_string(path).expect("schema file")).expect("json");
        jsonschema::options()
            .with_draft(Draft::Draft4)
            .build(&schema)
            .expect("compile SARIF schema")
    })
}

#[test]
fn multiline_docstring_ignore_does_not_create_unused_suppression() {
    let tmp = TempDir::new().unwrap();
    fs::write(
        tmp.path().join("app.py"),
        r#"async def handler() -> int:
    """Example usage:
    time.sleep(1)  # async-doctor: ignore[blocking-sleep]: example inside docstring
    # async-doctor: ignore[blocking-http]
    """
    raw = r'''
    # async-doctor: ignore[blocking-file]: raw multiline string
    '''
    return len(raw)
"#,
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
        "stderr={} stdout={}",
        String::from_utf8_lossy(&out.stderr),
        String::from_utf8_lossy(&out.stdout)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("requires a reason"),
        "docstring text must not trigger missing-reason warning: {stderr}"
    );
    let v: Value = serde_json::from_slice(&out.stdout).expect("json");
    assert_eq!(v["unusedSuppressions"].as_array().unwrap().len(), 0);
}

#[test]
fn multiline_call_trailing_ignore_and_decorator_above_ignore_suppress_findings() {
    let tmp = TempDir::new().unwrap();
    fs::write(
        tmp.path().join("app.py"),
        r#"import time

def dec(fn):
    return fn

async def multiline_call_ok() -> None:
    time.sleep(
        1,
    )  # async-doctor: ignore[blocking-sleep]: trailing comment on closing paren

# async-doctor: ignore[blocking-sleep]: comment above stacked decorators
@dec
@dec
async def decorated_entry_ok() -> None:
    time.sleep(1)
"#,
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
        "stderr={} stdout={}",
        String::from_utf8_lossy(&out.stderr),
        String::from_utf8_lossy(&out.stdout)
    );
    let v: Value = serde_json::from_slice(&out.stdout).expect("json");
    assert_eq!(v["findings"].as_array().unwrap().len(), 0, "{v}");
    assert_eq!(v["unusedSuppressions"].as_array().unwrap().len(), 0, "{v}");
}

#[test]
fn single_file_scan_ignores_sibling_unused_suppression_and_sibling_missing_reason() {
    let tmp = TempDir::new().unwrap();
    let clean = tmp.path().join("clean.py");
    fs::write(&clean, "async def handler() -> int:\n    return 1\n").unwrap();
    fs::write(
        tmp.path().join("sibling.py"),
        "# async-doctor: ignore[blocking-sleep]: stale in sibling\n# async-doctor: ignore[blocking-http]\nx = 1\n",
    )
    .unwrap();

    let out = bin()
        .args(["--format", "json"])
        .arg(&clean)
        .output()
        .expect("run single file");
    assert_eq!(
        out.status.code(),
        Some(0),
        "single-file scan must not fail on sibling unused suppression: stderr={} stdout={}",
        String::from_utf8_lossy(&out.stderr),
        String::from_utf8_lossy(&out.stdout)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("sibling.py"),
        "single-file scan must not warn on sibling.py: {stderr}"
    );
    let v: Value = serde_json::from_slice(&out.stdout).expect("json");
    assert_eq!(v["unusedSuppressions"].as_array().unwrap().len(), 0);
}

#[test]
fn changed_and_diff_filter_untouched_unused_suppressions_and_withstand_git_diff_config() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    git(root, &["init", "-b", "main"]);
    git(root, &["config", "user.email", "a@b.c"]);
    git(root, &["config", "user.name", "a"]);
    // Hostile user diff settings that break naive `+++ b/` hunk parsing.
    git(root, &["config", "diff.mnemonicPrefix", "true"]);
    git(root, &["config", "diff.noprefix", "true"]);

    fs::write(
        root.join("legacy.py"),
        "# async-doctor: ignore[blocking-sleep]: stale committed ignore\nx = 1\n",
    )
    .unwrap();
    fs::write(
        root.join("feature.py"),
        "import time\n\nasync def run() -> int:\n    return 1\n",
    )
    .unwrap();
    git(root, &["add", "."]);
    git(root, &["commit", "-m", "init"]);

    // Modify feature.py cleanly while legacy.py still has an untouched unused ignore.
    fs::write(
        root.join("feature.py"),
        "import time\n\nasync def run() -> int:\n    return 2\n",
    )
    .unwrap();

    for flag in ["--changed", "--diff"] {
        let out = bin()
            .args(["--format", "json", flag])
            .arg(root)
            .output()
            .expect("run");
        assert_eq!(
            out.status.code(),
            Some(0),
            "{flag} must ignore untouched unused suppressions: stderr={} stdout={}",
            String::from_utf8_lossy(&out.stderr),
            String::from_utf8_lossy(&out.stdout)
        );
        let v: Value = serde_json::from_slice(&out.stdout).expect("json");
        assert_eq!(v["unusedSuppressions"].as_array().unwrap().len(), 0);
    }

    // Now introduce a blocking call in feature.py; `--diff` must catch it even with
    // `diff.mnemonicPrefix=true` and `diff.noprefix=true`.
    fs::write(
        root.join("feature.py"),
        "import time\n\nasync def run() -> int:\n    time.sleep(1)\n    return 2\n",
    )
    .unwrap();
    let out = bin()
        .args(["--format", "json", "--diff"])
        .arg(root)
        .output()
        .expect("run diff");
    assert_eq!(out.status.code(), Some(1));
    let v: Value = serde_json::from_slice(&out.stdout).expect("json");
    assert_eq!(v["findings"].as_array().unwrap().len(), 1);
    assert_eq!(v["findings"][0]["entry"]["file"], "feature.py");
}

#[test]
fn fail_on_none_exits_zero_on_findings_and_two_on_error() {
    let dir = fixture("m1_multihop");
    let out = bin()
        .args(["--fail-on", "none", "--format", "json"])
        .arg(&dir)
        .output()
        .expect("run");
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: Value = serde_json::from_slice(&out.stdout).expect("json");
    assert!(!v["findings"].as_array().unwrap().is_empty());

    let bad = bin()
        .args(["--fail-on", "none", "/nonexistent/async-doctor-xyz"])
        .output()
        .expect("run");
    assert_eq!(bad.status.code(), Some(2));
}

#[test]
fn why_respects_inline_suppressions_supports_json_and_handles_spaces() {
    let tmp = TempDir::new().unwrap();
    let spaced_dir = tmp.path().join("my service").join("sub dir");
    fs::create_dir_all(&spaced_dir).unwrap();
    fs::write(
        spaced_dir.join("api routes.py"),
        r#"import time

async def suppressed_route() -> None:
    time.sleep(1)  # async-doctor: ignore[blocking-sleep]: ok here

async def active_route() -> None:
    time.sleep(2)
"#,
    )
    .unwrap();

    let project_root = tmp.path().join("my service");

    // Suppressed site at line 4 must NOT be reported by `why`.
    let sup_out = bin()
        .args([
            "why",
            "sub dir/api routes.py:4",
            "--format",
            "json",
            "--path",
        ])
        .arg(&project_root)
        .output()
        .expect("run why suppressed");
    assert_eq!(
        sup_out.status.code(),
        Some(0),
        "stderr={} stdout={}",
        String::from_utf8_lossy(&sup_out.stderr),
        String::from_utf8_lossy(&sup_out.stdout)
    );
    let sup_json: Value = serde_json::from_slice(&sup_out.stdout).expect("why json");
    assert_eq!(sup_json["schemaVersion"], 2);
    assert_eq!(sup_json["findings"].as_array().unwrap().len(), 0);

    // Scan JSON must emit a space-safe `why` string for line 7, and `why --format json` must return it.
    let scan_out = bin()
        .args(["--format", "json"])
        .arg(&project_root)
        .output()
        .expect("scan");
    assert_eq!(scan_out.status.code(), Some(1));
    let scan_json: Value = serde_json::from_slice(&scan_out.stdout).expect("scan json");
    let why_cmd = scan_json["findings"][0]["why"].as_str().unwrap();
    assert!(
        why_cmd.contains("\"sub dir/api routes.py:7\""),
        "why command must quote locations containing spaces: {why_cmd}"
    );

    let act_out = bin()
        .args([
            "why",
            "sub dir/api routes.py:7",
            "--format",
            "json",
            "--path",
        ])
        .arg(&project_root)
        .output()
        .expect("run why active");
    assert_eq!(act_out.status.code(), Some(1));
    let act_json: Value = serde_json::from_slice(&act_out.stdout).expect("why json");
    assert_eq!(act_json["location"]["file"], "sub dir/api routes.py");
    assert_eq!(act_json["location"]["line"], 7);
    assert_eq!(act_json["findings"].as_array().unwrap().len(), 1);
    assert_eq!(act_json["findings"][0]["rule"], "blocking-sleep");
    assert_eq!(act_json["rules"][0]["id"], "blocking-sleep");
}

#[test]
fn max_findings_preserves_full_sink_sites_in_grouped_text_and_markdown() {
    let dir = fixture("m4_max_findings");

    let text_out = bin()
        .args(["--max-findings", "1"])
        .arg(&dir)
        .output()
        .expect("run text");
    assert_eq!(text_out.status.code(), Some(1));
    let text = String::from_utf8_lossy(&text_out.stdout);
    let lead = text
        .lines()
        .find(|l| l.starts_with("Found "))
        .expect("lead");
    assert!(
        lead.contains("2 sink sites") && lead.contains("3 findings"),
        "grouped text headline must preserve full pre-cap counts, got: {lead}"
    );

    let md_out = bin()
        .args(["--format", "markdown", "--max-findings", "1"])
        .arg(&dir)
        .output()
        .expect("run markdown");
    assert_eq!(md_out.status.code(), Some(1));
    let md = String::from_utf8_lossy(&md_out.stdout);
    assert!(
        md.contains("Found **2** sink sites · **3** findings"),
        "markdown summary must preserve full pre-cap counts:\n{md}"
    );
}

#[test]
fn sarif_uses_canonical_rule_descriptors_rule_index_and_percent_encodes_spaces() {
    let tmp = TempDir::new().unwrap();
    let sub = tmp.path().join("space dir");
    fs::create_dir_all(&sub).unwrap();
    fs::write(
        sub.join("my routes.py"),
        "import time\n\nasync def handler() -> None:\n    time.sleep(1)\n",
    )
    .unwrap();

    let out = bin()
        .args(["--format", "sarif"])
        .arg(tmp.path())
        .output()
        .expect("run sarif");
    assert_eq!(out.status.code(), Some(1));
    let doc: Value = serde_json::from_slice(&out.stdout).expect("sarif json");
    sarif_validator()
        .validate(&doc)
        .expect("SARIF with spaces must validate against 2.1.0 schema");

    let rules = doc["runs"][0]["tool"]["driver"]["rules"]
        .as_array()
        .unwrap();
    assert_eq!(rules.len(), 1);
    let help_text = rules[0]["help"]["text"].as_str().unwrap();
    assert!(
        !help_text.contains("my routes.py"),
        "global SARIF rule descriptor must not embed instance-specific file:line: {help_text}"
    );

    let result = &doc["runs"][0]["results"][0];
    assert_eq!(result["ruleIndex"], 0);
    let uri = result["locations"][0]["physicalLocation"]["artifactLocation"]["uri"]
        .as_str()
        .unwrap();
    assert!(
        uri.contains("space%20dir/my%20routes.py") && !uri.contains(' '),
        "SARIF artifact URI must percent-encode spaces, got: {uri}"
    );
}
