//! E2E tests for P1: 0-100 health score (`Report::score` + `--score`) and
//! `async-doctor install-skill`.

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
        .join("../../fixtures")
        .join(name)
}

#[test]
fn score_flag_outputs_100_on_clean_and_lower_on_findings() {
    let clean = bin()
        .arg("--score")
        .arg(fixture("negative_sync.py"))
        .output()
        .expect("run --score on clean fixture");
    assert_eq!(clean.status.code(), Some(0));
    let clean_stdout = String::from_utf8_lossy(&clean.stdout);
    assert_eq!(clean_stdout.trim(), "100");

    let dirty = bin()
        .arg("--score")
        .arg(fixture("m1_multihop"))
        .output()
        .expect("run --score on dirty fixture");
    assert_eq!(dirty.status.code(), Some(1));
    let score: u32 = String::from_utf8_lossy(&dirty.stdout)
        .trim()
        .parse()
        .expect("numeric score on stdout");
    assert!(
        score < 85,
        "expected score < 85 when findings are present, got {score}"
    );
}

#[test]
fn score_recomputes_after_baseline_suppression_and_appears_in_json_text_markdown() {
    let dir = tempfile::tempdir().expect("tempdir");
    let py = dir.path().join("app.py");
    std::fs::write(
        &py,
        "import time\n\nasync def handler():\n    time.sleep(1)\n",
    )
    .expect("write app.py");

    let json_out = bin()
        .args(["--format", "json"])
        .arg(dir.path())
        .output()
        .expect("run json");
    assert_eq!(json_out.status.code(), Some(1));
    let v: serde_json::Value = serde_json::from_slice(&json_out.stdout).expect("parse json");
    let initial_score = v["score"].as_u64().expect("score field in json");
    assert_eq!(
        initial_score, 59,
        "1 blocking-sleep finding affecting 1/1 async entry should score 59"
    );

    let text_out = bin().arg(dir.path()).output().expect("run text");
    let text_stdout = String::from_utf8_lossy(&text_out.stdout);
    assert!(
        text_stdout.contains("Score: 59/100 (Critical)"),
        "expected score in text footer, got:\n{text_stdout}"
    );

    let md_out = bin()
        .args(["--format", "markdown"])
        .arg(dir.path())
        .output()
        .expect("run markdown");
    let md_stdout = String::from_utf8_lossy(&md_out.stdout);
    assert!(
        md_stdout.contains("| **Score** | **59 / 100** (Critical) |"),
        "expected score row in markdown table, got:\n{md_stdout}"
    );

    // Write baseline and verify --baseline restores score to 100.
    let baseline = dir.path().join("baseline.json");
    let write_bl = bin()
        .arg("--write-baseline")
        .arg(&baseline)
        .arg(dir.path())
        .output()
        .expect("write baseline");
    assert_eq!(write_bl.status.code(), Some(0));

    let suppressed = bin()
        .arg("--score")
        .arg("--baseline")
        .arg(&baseline)
        .arg(dir.path())
        .output()
        .expect("run --score with --baseline");
    assert_eq!(suppressed.status.code(), Some(0));
    assert_eq!(String::from_utf8_lossy(&suppressed.stdout).trim(), "100");
}

#[test]
fn install_skill_writes_agents_skill_and_claude_symlink() {
    let dir = tempfile::tempdir().expect("tempdir");
    let out = bin()
        .args(["install-skill", "--path"])
        .arg(dir.path())
        .output()
        .expect("run install-skill");
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );

    let canonical = dir.path().join(".agents/skills/async-doctor/SKILL.md");
    assert!(canonical.is_file(), "expected {}", canonical.display());
    let content = std::fs::read_to_string(&canonical).expect("read SKILL.md");
    assert!(content.starts_with("---\nname: async-doctor\n"));

    let claude_link = dir.path().join(".claude/skills/async-doctor");
    let linked_skill = claude_link.join("SKILL.md");
    assert!(
        linked_skill.is_file(),
        "expected {} to resolve to SKILL.md",
        linked_skill.display()
    );
    #[cfg(unix)]
    {
        let target =
            std::fs::read_link(&claude_link).expect("read_link .claude/skills/async-doctor");
        assert_eq!(target, PathBuf::from("../../.agents/skills/async-doctor"));
    }

    // Re-running install-skill is idempotent when content matches.
    let out_again = bin()
        .args(["install-skill", "--path"])
        .arg(dir.path())
        .output()
        .expect("re-run install-skill");
    assert_eq!(out_again.status.code(), Some(0));
    assert!(
        String::from_utf8_lossy(&out_again.stdout).contains("installed skill:"),
        "idempotent re-run should still report install: {}",
        String::from_utf8_lossy(&out_again.stdout)
    );
}

#[test]
fn install_skill_skips_divergent_skill_without_force() {
    let dir = tempfile::tempdir().expect("tempdir");
    let skill_dir = dir.path().join(".agents/skills/async-doctor");
    std::fs::create_dir_all(&skill_dir).unwrap();
    let skill_file = skill_dir.join("SKILL.md");
    std::fs::write(&skill_file, "user customized skill\n").unwrap();

    let out = bin()
        .args(["install-skill", "--path"])
        .arg(dir.path())
        .output()
        .expect("run install-skill");
    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("skip:") && stdout.contains("--force"),
        "expected skip message, got: {stdout}"
    );
    assert_eq!(
        std::fs::read_to_string(&skill_file).unwrap(),
        "user customized skill\n",
        "divergent SKILL.md must be left untouched without --force"
    );
    assert!(
        !dir.path().join(".claude/skills/async-doctor").exists(),
        "Claude link must not be created when skill install is skipped"
    );
}

#[test]
fn install_skill_force_overwrites_divergent_skill() {
    let dir = tempfile::tempdir().expect("tempdir");
    let skill_dir = dir.path().join(".agents/skills/async-doctor");
    std::fs::create_dir_all(&skill_dir).unwrap();
    let skill_file = skill_dir.join("SKILL.md");
    std::fs::write(&skill_file, "user customized skill\n").unwrap();

    let out = bin()
        .args(["install-skill", "--force", "--path"])
        .arg(dir.path())
        .output()
        .expect("run install-skill --force");
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let content = std::fs::read_to_string(&skill_file).unwrap();
    assert!(
        content.starts_with("---\nname: async-doctor\n"),
        "--force must overwrite divergent SKILL.md"
    );
    #[cfg(unix)]
    {
        let target = std::fs::read_link(dir.path().join(".claude/skills/async-doctor")).unwrap();
        assert_eq!(target, PathBuf::from("../../.agents/skills/async-doctor"));
    }
}

#[test]
fn install_skill_updates_managed_claude_directory_copy() {
    // Simulates the Windows directory-copy install: a real directory that
    // contains only SKILL.md. Re-runs (especially --force) must update that
    // copy in place without deleting the directory.
    let dir = tempfile::tempdir().expect("tempdir");
    let agents = dir.path().join(".agents/skills/async-doctor");
    std::fs::create_dir_all(&agents).unwrap();
    std::fs::write(agents.join("SKILL.md"), "stale agents skill\n").unwrap();
    let claude = dir.path().join(".claude/skills/async-doctor");
    std::fs::create_dir_all(&claude).unwrap();
    std::fs::write(claude.join("SKILL.md"), "stale claude copy\n").unwrap();

    let out = bin()
        .args(["install-skill", "--force", "--path"])
        .arg(dir.path())
        .output()
        .expect("run install-skill --force");
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("updated Claude skill copy") || stdout.contains("installed skill:"),
        "expected managed-copy update message, got: {stdout}"
    );
    let agents_content = std::fs::read_to_string(agents.join("SKILL.md")).unwrap();
    let claude_content = std::fs::read_to_string(claude.join("SKILL.md")).unwrap();
    assert!(
        agents_content.starts_with("---\nname: async-doctor\n"),
        "agents SKILL.md must be refreshed"
    );
    assert_eq!(
        agents_content, claude_content,
        "managed Claude copy must match the agents skill after --force"
    );
    assert!(
        claude.is_dir()
            && !std::fs::symlink_metadata(&claude)
                .unwrap()
                .file_type()
                .is_symlink(),
        "managed copy must remain a real directory"
    );
}

#[test]
fn install_skill_never_deletes_real_claude_skill_directory() {
    let dir = tempfile::tempdir().expect("tempdir");
    let claude_dir = dir.path().join(".claude/skills/async-doctor");
    std::fs::create_dir_all(&claude_dir).unwrap();
    let user_file = claude_dir.join("NOTES.md");
    std::fs::write(&user_file, "keep me\n").unwrap();

    let out = bin()
        .args(["install-skill", "--path"])
        .arg(dir.path())
        .output()
        .expect("run install-skill");
    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("skip:") && stdout.contains("real directory"),
        "expected skip for real directory, got: {stdout}"
    );
    assert_eq!(std::fs::read_to_string(&user_file).unwrap(), "keep me\n");

    let out_force = bin()
        .args(["install-skill", "--force", "--path"])
        .arg(dir.path())
        .output()
        .expect("run install-skill --force");
    assert_eq!(out_force.status.code(), Some(0));
    let force_stdout = String::from_utf8_lossy(&out_force.stdout);
    assert!(
        force_stdout.contains("never deletes") || force_stdout.contains("real directory"),
        "force must still refuse to delete user directory: {force_stdout}"
    );
    assert_eq!(
        std::fs::read_to_string(&user_file).unwrap(),
        "keep me\n",
        "user files under a real .claude/skills/async-doctor directory must survive --force"
    );
    let skill = dir.path().join(".agents/skills/async-doctor/SKILL.md");
    assert!(
        skill.is_file(),
        "--force should still write the agents skill file"
    );
}

#[test]
fn why_subcommand_matches_constructor_init_and_with_open_write_lines() {
    let dir = tempfile::tempdir().expect("tempdir");
    let py = dir.path().join("app.py");
    std::fs::write(
        &py,
        concat!(
            "from pathlib import Path\n",
            "\n",
            "class Worker:\n",
            "    def __init__(self, home: Path) -> None:\n",
            "        (home / \"logs\").mkdir(parents=True)\n",
            "\n",
            "async def start(home: Path, out: Path) -> None:\n",
            "    Worker(home)\n",
            "    with open(out, \"w\", encoding=\"utf-8\") as f:\n",
            "        f.write(\"ok\")\n",
        ),
    )
    .expect("write app.py");

    // Constructor sink line (line 5: mkdir) and call site (line 8: Worker(home))
    for line in [5, 8, 9, 10] {
        let out = bin()
            .args(["why", &format!("app.py:{line}"), "--path"])
            .arg(dir.path())
            .output()
            .expect("run why");
        assert_eq!(
            out.status.code(),
            Some(1),
            "expected `why app.py:{line}` to match a finding (exit 1), stderr={}",
            String::from_utf8_lossy(&out.stderr)
        );
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            stdout.contains(&format!("Evidence for app.py:{line} (1 finding(s)"))
                && stdout.contains("blocking-file"),
            "expected blocking-file evidence in `why app.py:{line}` output:\n{stdout}"
        );
    }
}
