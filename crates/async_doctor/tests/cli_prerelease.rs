//! CLI regressions from the pre-release audit (bare paths, empty dirs).

use std::fs;
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

fn fixture_pkg() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/prerelease_edge/pkg")
        .canonicalize()
        .expect("prerelease_edge fixture")
}

#[test]
fn bare_relative_file_path_scans() {
    let pkg = fixture_pkg();
    let out = bin()
        .current_dir(&pkg)
        .args(["good.py", "--format", "json"])
        .output()
        .expect("run");
    assert_eq!(
        out.status.code(),
        Some(1),
        "stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("blocking-sleep"), "{stdout}");
    assert!(!stdout.contains("directory walk failed"), "{stdout}");
}

#[test]
fn empty_dir_message_text_and_json() {
    let dir = tempfile::tempdir().unwrap();
    let text = bin()
        .args([dir.path().to_str().unwrap(), "--format", "text"])
        .output()
        .expect("run");
    assert_eq!(text.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&text.stdout);
    assert!(stdout.contains("No Python files were found."), "{stdout}");
    assert!(!stdout.contains("No findings."), "{stdout}");

    let json = bin()
        .args([dir.path().to_str().unwrap(), "--format", "json"])
        .output()
        .expect("run");
    assert_eq!(json.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&json.stdout);
    assert!(stdout.contains("No Python files were found."), "{stdout}");
}

#[test]
fn no_py_dir_message() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("readme.txt"), "hi\n").unwrap();
    let out = bin()
        .args([dir.path().to_str().unwrap(), "--format", "text"])
        .output()
        .expect("run");
    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("No Python files were found."), "{stdout}");
}
