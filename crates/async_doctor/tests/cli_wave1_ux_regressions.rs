//! Wave 1 CLI & UX regressions:
//! - Inline ignore trailing `line + 1` bleed vs standalone preceding ignore
//! - Stacked pragmas (`# type: ignore` / `# noqa`) & multi-rule `ignore[a, b]: reason`
//! - `why` deduplicated rule docs & >5 chain capping unless `--expand`
//! - Compact JSON on non-TTY stdout
//! - Grouped text & Markdown caps, `Top site:` reachability, and `complete=false` stdout warning
//! - Fast `--changed` zero-Python-diff short-circuit before `scan_project`
//! - Packaging (`pyproject.toml` sdist `SKILL.md`), `action.yml`, `.pre-commit-hooks.yaml`, `release.yml`, `SKILL.md` decision tree

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::Value;
use tempfile::TempDir;

static CACHE_SEQ: AtomicU64 = AtomicU64::new(0);

fn bin() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_async-doctor"));
    cmd.env("NO_COLOR", "1");
    cmd.env_remove("VIRTUAL_ENV").env_remove("CONDA_PREFIX");
    let seq = CACHE_SEQ.fetch_add(1, Ordering::Relaxed);
    let cache = std::env::temp_dir().join(format!(
        "async-doctor-w1-ux-cache-{}-{seq}",
        std::process::id()
    ));
    let _ = fs::create_dir_all(&cache);
    cmd.env("XDG_CACHE_HOME", cache);
    cmd
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("workspace root")
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
fn trailing_ignore_does_not_bleed_to_next_line_while_standalone_preceding_ignore_works() {
    let tmp = TempDir::new().unwrap();
    fs::write(
        tmp.path().join("app.py"),
        concat!(
            "import time\n",
            "\n",
            "async def handler() -> None:\n",
            "    time.sleep(0.01)  # async-doctor: ignore[blocking-sleep]: intentional short poll\n",
            "    time.sleep(10)\n",
            "    # async-doctor: ignore[blocking-sleep]: standalone preceding comment\n",
            "    time.sleep(0.02)\n",
        ),
    )
    .unwrap();

    let out = bin()
        .args(["--format", "json"])
        .arg(tmp.path())
        .output()
        .expect("run");
    assert_eq!(
        out.status.code(),
        Some(1),
        "un-annotated sleep on line 5 must NOT be suppressed by trailing ignore on line 4: stdout={}",
        String::from_utf8_lossy(&out.stdout)
    );
    let v: Value = serde_json::from_slice(&out.stdout).expect("json");
    let findings = v["findings"].as_array().unwrap();
    assert_eq!(findings.len(), 1, "expected only line 5 finding: {v}");
    let sink_line = findings[0]["chain"].as_array().unwrap().last().unwrap()["line"]
        .as_u64()
        .unwrap();
    assert_eq!(sink_line, 5);
    assert!(v["unusedSuppressions"].as_array().unwrap().is_empty());
}

#[test]
fn stacked_pragmas_and_multi_rule_ignore_with_per_rule_unused_tracking() {
    let tmp = TempDir::new().unwrap();
    fs::write(
        tmp.path().join("stacked.py"),
        concat!(
            "import subprocess\n",
            "import time\n",
            "\n",
            "async def stacked_pragmas() -> None:\n",
            "    time.sleep(1)  # type: ignore[arg-type]  # async-doctor: ignore[blocking-sleep]: after type ignore\n",
            "    time.sleep(2)  # noqa: E501  # async-doctor: ignore[blocking-sleep]: after noqa\n",
            "    time.sleep(3)  # async-doctor: ignore[blocking-sleep]: see issue #42  # noqa: E501\n",
            "\n",
            "# async-doctor: ignore[blocking-sleep, blocking-subprocess]: legacy cli entry\n",
            "async def multi_rule_both_used() -> None:\n",
            "    time.sleep(1)\n",
            "    subprocess.run([\"true\"], check=False)\n",
        ),
    )
    .unwrap();

    let clean = bin()
        .args(["--format", "json"])
        .arg(tmp.path())
        .output()
        .expect("run stacked");
    assert_eq!(
        clean.status.code(),
        Some(0),
        "stacked pragmas and multi-rule ignore must suppress all findings: stderr={} stdout={}",
        String::from_utf8_lossy(&clean.stderr),
        String::from_utf8_lossy(&clean.stdout)
    );
    let v: Value = serde_json::from_slice(&clean.stdout).expect("json");
    assert!(v["findings"].as_array().unwrap().is_empty());
    assert!(v["unusedSuppressions"].as_array().unwrap().is_empty());

    // Partial match in multi-rule ignore: used rule suppresses finding; unused rule is reported.
    let partial = TempDir::new().unwrap();
    fs::write(
        partial.path().join("partial.py"),
        concat!(
            "import time\n",
            "\n",
            "async def partial_multi() -> None:\n",
            "    time.sleep(1)  # async-doctor: ignore[blocking-sleep, blocking-subprocess]: partial reason  # type: ignore[misc]\n",
        ),
    )
    .unwrap();
    let p_out = bin()
        .args(["--format", "json"])
        .arg(partial.path())
        .output()
        .expect("run partial");
    assert_eq!(p_out.status.code(), Some(1));
    let pv: Value = serde_json::from_slice(&p_out.stdout).expect("json");
    assert!(pv["findings"].as_array().unwrap().is_empty(), "{pv}");
    let unused = pv["unusedSuppressions"].as_array().unwrap();
    assert_eq!(unused.len(), 1, "{pv}");
    assert_eq!(unused[0]["rule"], "blocking-subprocess");
    assert_eq!(unused[0]["reason"], "partial reason");
}

#[test]
fn why_deduplicates_rule_docs_and_caps_chains_over_five_unless_expand() {
    let tmp = TempDir::new().unwrap();
    fs::write(
        tmp.path().join("helpers.py"),
        "import time\n\ndef slow_io() -> None:\n    time.sleep(1)\n",
    )
    .unwrap();
    let mut routes = String::from("from helpers import slow_io\n\n");
    for i in 0..8 {
        routes.push_str(&format!(
            "async def route_{i}() -> None:\n    slow_io()\n\n"
        ));
    }
    fs::write(tmp.path().join("routes.py"), routes).unwrap();

    let out = bin()
        .args(["why", "helpers.py:4", "--path"])
        .arg(tmp.path())
        .output()
        .expect("run why");
    assert_eq!(out.status.code(), Some(1));
    let stdout = String::from_utf8_lossy(&out.stdout);
    let url_needle =
        "https://github.com/maolonglong/async-doctor/blob/main/docs/rules.md#blocking-sleep";
    // Each finding prints `docs: <url>` once (3 shown chains) + RuleDoc at bottom prints `Docs: <url>` once.
    let rule_doc_headers = stdout.matches("blocking-sleep — ").count();
    assert_eq!(
        rule_doc_headers, 1,
        "RuleDoc explain block must appear at most once in `why` text output:\n{stdout}"
    );
    assert_eq!(
        stdout.matches("Hop resolution:").count(),
        3,
        "expected first 3 chains when >5 findings share sink/rule without --expand:\n{stdout}"
    );
    assert!(
        stdout.contains("... and 5 more entries (pass --expand to list all)"),
        "expected concise remaining-entries summary:\n{stdout}"
    );
    assert!(stdout.contains(url_needle));

    // With --expand, all 8 chains print, and RuleDoc still prints once.
    let exp_out = bin()
        .args(["why", "helpers.py:4", "--expand", "--path"])
        .arg(tmp.path())
        .output()
        .expect("run why --expand");
    assert_eq!(exp_out.status.code(), Some(1));
    let exp_stdout = String::from_utf8_lossy(&exp_out.stdout);
    assert_eq!(exp_stdout.matches("Hop resolution:").count(), 8);
    assert!(!exp_stdout.contains("more entries (pass --expand"));
    assert_eq!(exp_stdout.matches("blocking-sleep — ").count(), 1);

    // JSON schema stays intact with all 8 findings and 1 deduplicated rule entry.
    let json_out = bin()
        .args(["why", "helpers.py:4", "--format", "json", "--path"])
        .arg(tmp.path())
        .output()
        .expect("run why json");
    assert_eq!(json_out.status.code(), Some(1));
    let jv: Value = serde_json::from_slice(&json_out.stdout).expect("why json");
    assert_eq!(jv["schemaVersion"], 2);
    assert_eq!(jv["findings"].as_array().unwrap().len(), 8);
    assert_eq!(jv["rules"].as_array().unwrap().len(), 1);
}

#[test]
fn json_stdout_is_compact_single_line_when_piped() {
    let dir = repo_root().join("fixtures/m1_multihop");
    let out = bin()
        .args(["--format", "json"])
        .arg(&dir)
        .output()
        .expect("run json");
    assert_eq!(out.status.code(), Some(1));
    let raw = std::str::from_utf8(&out.stdout).expect("utf8");
    assert_eq!(
        raw.trim_end().lines().count(),
        1,
        "non-TTY JSON stdout must be compact single-line JSON, got:\n{raw}"
    );
    let v: Value = serde_json::from_str(raw).expect("valid json");
    assert_eq!(v["schemaVersion"], 2);
}

#[test]
fn grouped_text_and_markdown_cap_output_and_surface_top_site_and_incomplete_note() {
    let tmp = TempDir::new().unwrap();
    // Include a syntax-error file so `report.complete == false`.
    fs::write(tmp.path().join("broken.py"), "def broken(:\n").unwrap();

    // Shared choke helper hitting one primary sink across 4 async entries,
    // plus 11 distinct diagnostic groups (12 total) and 12 dangling-task advice items.
    fs::write(
        tmp.path().join("choke.py"),
        "import time\n\ndef shared_gate() -> None:\n    sink_call()\n\ndef sink_call() -> None:\n    time.sleep(1)\n",
    )
    .unwrap();
    let mut app = String::from(
        "import asyncio\nimport ftplib\nimport glob\nimport os\nimport pathlib\nimport smtplib\nimport socket\nimport sqlite3\nimport subprocess\nimport tarfile\nimport threading\nimport zipfile\nfrom choke import shared_gate\n\nasync def bg() -> None:\n    await asyncio.sleep(0)\n\n",
    );
    for i in 0..4 {
        app.push_str(&format!(
            "async def fanout_{i}() -> None:\n    shared_gate()\n\n"
        ));
    }
    let extra_sinks = [
        "subprocess.run(['true'], check=False)",
        "socket.create_connection(('127.0.0.1', 80))",
        "open('a.txt', 'r', encoding='utf-8').read()",
        "pathlib.Path('.').glob('*.py')",
        "glob.glob('*.py')",
        "sqlite3.connect(':memory:')",
        "threading.Lock().acquire()",
        "zipfile.ZipFile('a.zip')",
        "tarfile.open('a.tar')",
        "smtplib.SMTP('localhost')",
        "ftplib.FTP('localhost')",
    ];
    for (idx, call) in extra_sinks.iter().enumerate() {
        app.push_str(&format!("async def extra_{idx}() -> None:\n    {call}\n\n"));
    }
    app.push_str("async def advice_flood() -> None:\n");
    for _ in 0..12 {
        app.push_str("    asyncio.create_task(bg())\n");
    }
    fs::write(tmp.path().join("app.py"), app).unwrap();

    let text_out = bin().arg(tmp.path()).output().expect("run text");
    assert_eq!(text_out.status.code(), Some(1));
    let text = String::from_utf8_lossy(&text_out.stdout);
    assert!(
        text.contains("complete=false"),
        "text stdout must surface prominent incomplete-scan warning:\n{text}"
    );
    assert!(
        text.lines()
            .any(|line| { line.contains("Top site:") && line.contains("reached by 4 findings") }),
        "grouped text must surface observed reachability without promising a repair:\n{text}"
    );
    assert!(
        text.contains("more diagnostic group(s)"),
        "grouped text must cap diagnostic groups unless --expand:\n{text}"
    );

    let md_out = bin()
        .args(["--format", "markdown"])
        .arg(tmp.path())
        .output()
        .expect("run markdown");
    assert_eq!(md_out.status.code(), Some(1));
    let md = String::from_utf8_lossy(&md_out.stdout);
    assert!(
        md.contains("complete=false"),
        "markdown stdout must surface prominent incomplete-scan warning:\n{md}"
    );
    assert!(
        md.contains("showing top 10") || md.contains("more advice"),
        "markdown must cap advice items unless --expand:\n{md}"
    );

    let md_exp = bin()
        .args(["--format", "markdown", "--expand"])
        .arg(tmp.path())
        .output()
        .expect("run markdown --expand");
    let md_exp_text = String::from_utf8_lossy(&md_exp.stdout);
    assert!(
        !md_exp_text.contains("showing top 10"),
        "markdown --expand must show all advice items:\n{md_exp_text}"
    );
}

#[test]
fn changed_short_circuits_before_scan_when_zero_python_or_config_files_touched() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    git(root, &["init", "-b", "main"]);
    git(root, &["config", "user.email", "a@b.c"]);
    git(root, &["config", "user.name", "a"]);
    // Syntax error in broken.py proves whether `scan_project` actually ran (`complete=false` if scanned).
    fs::write(root.join("broken.py"), "def broken(:\n").unwrap();
    fs::write(
        root.join("app.py"),
        "import time\n\nasync def handler() -> None:\n    time.sleep(1)\n",
    )
    .unwrap();
    fs::write(root.join("README.md"), "# docs\n").unwrap();
    fs::write(root.join("pyproject.toml"), "[project]\nname = \"demo\"\n").unwrap();
    git(root, &["add", "."]);
    git(root, &["commit", "-m", "init"]);

    // Touch only non-Python file -> must short-circuit with clean empty Report (`complete=true`, `fileCount=0`).
    fs::write(root.join("README.md"), "# updated docs\n").unwrap();
    let out = bin()
        .args(["--format", "json", "--changed"])
        .arg(root)
        .env_remove("GITHUB_BASE_REF")
        .output()
        .expect("run changed docs-only");
    assert_eq!(out.status.code(), Some(0));
    let v: Value = serde_json::from_slice(&out.stdout).expect("json");
    assert_eq!(
        v["complete"], true,
        "docs-only diff must short-circuit before scanning broken.py: {v}"
    );
    assert_eq!(v["fileCount"], 0);
    assert!(v["findings"].as_array().unwrap().is_empty());

    // Touch Python config file (`pyproject.toml`) -> must NOT short-circuit (`broken.py` is scanned -> `complete=false`).
    fs::write(
        root.join("pyproject.toml"),
        "[project]\nname = \"demo-v2\"\n",
    )
    .unwrap();
    let cfg_out = bin()
        .args(["--format", "json", "--changed"])
        .arg(root)
        .env_remove("GITHUB_BASE_REF")
        .output()
        .expect("run changed config");
    let cv: Value = serde_json::from_slice(&cfg_out.stdout).expect("json");
    assert_eq!(
        cv["complete"], false,
        "pyproject.toml change must run full scan rather than short-circuiting: {cv}"
    );
}

#[test]
fn packaging_hooks_action_release_gate_and_skill_decision_tree_invariants() {
    let root = repo_root();

    let pyproject = fs::read_to_string(root.join("pyproject.toml")).expect("pyproject.toml");
    assert!(
        pyproject.contains("skills/async-doctor/SKILL.md") && pyproject.contains("sdist"),
        "pyproject.toml must include skills/async-doctor/SKILL.md in sdist"
    );
    assert!(
        !pyproject.contains("\"skills/**\""),
        "pyproject.toml must not exclude skills/** from sdist"
    );

    let hooks =
        fs::read_to_string(root.join(".pre-commit-hooks.yaml")).expect(".pre-commit-hooks.yaml");
    assert!(hooks.contains("id: async-doctor"));
    assert!(hooks.contains("entry: async-doctor"));

    let action = fs::read_to_string(root.join("action.yml")).expect("action.yml");
    assert!(
        action.contains("using: \"composite\"")
            || action.contains("using: 'composite'")
            || action.contains("using: composite")
    );
    assert!(action.contains("sarif"));
    assert!(action.contains("args"));

    let release =
        fs::read_to_string(root.join(".github/workflows/release.yml")).expect("release.yml");
    assert!(release.contains("cargo test --workspace"));
    assert!(release.contains("scripts/check_docs.py"));
    assert!(release.contains("scripts/check_rust_file_length.sh"));

    let skill = fs::read_to_string(root.join("skills/async-doctor/SKILL.md")).expect("SKILL.md");
    assert!(skill.contains("Decision Tree"));
    assert!(skill.contains("sinkSites"));
    assert!(skill.contains("session.execute"));
}
