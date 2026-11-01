//! CLI integration tests for P0 UX: ANSI styling, scan footer, framework banner, and `--format markdown`.

use std::fs;
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
fn text_footer_prints_scanned_files_entries_and_elapsed_ms() {
    let dir = fixture("m1_multihop");
    let out = bin()
        .env_remove("NO_COLOR")
        .env_remove("CLICOLOR_FORCE")
        .env_remove("FORCE_COLOR")
        .arg(&dir)
        .output()
        .expect("run");
    assert_eq!(out.status.code(), Some(1));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        !stdout.contains("\x1b["),
        "piped stdout without force-color must not emit ANSI escapes: {stdout:?}"
    );
    assert!(
        stdout.contains("Scanned ") && stdout.contains(" file") && stdout.contains(" async entr"),
        "expected scan summary footer in text output: {stdout}"
    );
}

#[test]
fn text_ansi_styling_respects_clicolor_force_and_no_color() {
    let dir = fixture("m1_multihop");

    // Forced color emits ANSI escape sequences.
    let colored = bin()
        .env_remove("NO_COLOR")
        .env("CLICOLOR_FORCE", "1")
        .arg(&dir)
        .output()
        .expect("run");
    assert_eq!(colored.status.code(), Some(1));
    let colored_stdout = String::from_utf8_lossy(&colored.stdout);
    assert!(
        colored_stdout.contains("\x1b["),
        "CLICOLOR_FORCE=1 must emit ANSI escape codes: {colored_stdout:?}"
    );

    // NO_COLOR takes precedence over CLICOLOR_FORCE / FORCE_COLOR.
    let no_color = bin()
        .env("CLICOLOR_FORCE", "1")
        .env("FORCE_COLOR", "1")
        .env("NO_COLOR", "1")
        .arg(&dir)
        .output()
        .expect("run");
    assert_eq!(no_color.status.code(), Some(1));
    let plain_stdout = String::from_utf8_lossy(&no_color.stdout);
    assert!(
        !plain_stdout.contains("\x1b["),
        "NO_COLOR=1 must suppress all ANSI escape codes: {plain_stdout:?}"
    );
}

#[test]
fn environment_banner_and_json_report_detected_frameworks() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    fs::write(root.join("app.py"), "async def handler():\n    return 1\n").unwrap();

    let sp = root.join(".venv/lib/python3.12/site-packages");
    fs::create_dir_all(sp.join("fastapi-0.115.0.dist-info")).unwrap();
    fs::create_dir_all(sp.join("httpx-0.27.2.dist-info")).unwrap();
    fs::write(
        root.join(".venv/pyvenv.cfg"),
        "home = /usr/bin\nversion = 3.12.0\n",
    )
    .unwrap();

    let json_out = bin()
        .env_remove("VIRTUAL_ENV")
        .env_remove("CONDA_PREFIX")
        .args(["--format", "json"])
        .arg(root)
        .output()
        .expect("run json");
    assert_eq!(json_out.status.code(), Some(0));
    let v: serde_json::Value = serde_json::from_slice(&json_out.stdout).expect("json");
    let fws = v["environment"]["frameworks"]
        .as_array()
        .expect("environment.frameworks array");
    let fw_strs: Vec<&str> = fws.iter().filter_map(|x| x.as_str()).collect();
    assert!(
        fw_strs.contains(&"fastapi 0.115.0") && fw_strs.contains(&"httpx 0.27.2"),
        "expected fastapi and httpx in frameworks, got {fw_strs:?}"
    );

    let text_out = bin()
        .env_remove("VIRTUAL_ENV")
        .env_remove("CONDA_PREFIX")
        .arg(root)
        .output()
        .expect("run text");
    assert_eq!(text_out.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&text_out.stdout);
    assert!(
        stdout.contains("fastapi 0.115.0") && stdout.contains("httpx 0.27.2"),
        "environment banner must show detected frameworks: {stdout}"
    );
}

#[test]
fn markdown_format_emits_gfm_tables_and_details_with_exit_code() {
    let dir = fixture("m1_multihop");
    let out = bin()
        .args(["--format", "markdown"])
        .arg(&dir)
        .output()
        .expect("run markdown");
    assert_eq!(
        out.status.code(),
        Some(1),
        "stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let md = String::from_utf8_lossy(&out.stdout);
    assert!(
        md.contains("# async-doctor"),
        "missing markdown heading: {md}"
    );
    assert!(
        md.contains("| Rule |") || md.contains("| Sink site |"),
        "missing markdown summary table: {md}"
    );
    assert!(
        md.contains("<details>") && md.contains("</details>"),
        "missing collapsible details block: {md}"
    );
    assert!(md.contains("blocking-sleep"), "missing rule id: {md}");

    let clean = fixture("negative_sync.py");
    let clean_out = bin()
        .args(["--format", "markdown"])
        .arg(&clean)
        .output()
        .expect("run clean markdown");
    assert_eq!(clean_out.status.code(), Some(0));
    let clean_md = String::from_utf8_lossy(&clean_out.stdout);
    assert!(
        clean_md.contains("No findings"),
        "clean markdown report must state No findings: {clean_md}"
    );
}
