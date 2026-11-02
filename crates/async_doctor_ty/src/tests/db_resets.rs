//! Regression: default parallel extract must not periodically `fresh_empty()`.
//!
//! Periodic rebuilds allocate independent `Arc<Zalsa>` arenas; retaining them
//! until scan end caused OOM on large monorepos (see
//! `docs/notes/2026-11-salsa-fresh-empty-rss.md`).

use std::fs;

use crate::scan::scan_path_counting_fresh_empty;

use super::helpers::temp_project;

#[test]
fn default_parallel_path_bounds_fresh_empty_calls() {
    // Enough files that the old pass-1 (256) / pass-2 (160) intervals would
    // fire several times each. Default path should only rebuild once after
    // pass 1 (to release the discovery DB before follow) plus rare follow
    // resets — not O(files / interval).
    let mut files = Vec::with_capacity(400);
    let mut owned: Vec<(String, String)> = Vec::with_capacity(400);
    for i in 0..400 {
        let name = format!("m{i:03}.py");
        let body = format!("async def entry_{i}():\n    return {i}\n");
        owned.push((name, body));
    }
    for (name, body) in &owned {
        files.push((name.as_str(), body.as_str()));
    }
    let dir = temp_project(&files);
    // Ensure no site-packages follow work that would add follow resets.
    let _ = fs::remove_dir_all(dir.join(".venv"));

    // Counter reset + read happen under the same hermetic env lock as the
    // scan, so plain multi-threaded `cargo test` cannot race other scans.
    let (report, resets) = scan_path_counting_fresh_empty(&dir).expect("scan");

    assert!(
        report.complete,
        "scan should complete: skipped={:?}",
        report
    );
    // One post-pass-1 rebuild is expected; allow a small cushion for panic-free
    // follow that does nothing. The old intervals would exceed ~400/160 ≈ 2
    // per worker × N workers ≫ 8.
    assert!(
        resets <= 4,
        "default path must not periodically fresh_empty (got {resets} resets for 400 files)"
    );
}

#[test]
fn multi_wave_pass2_resolves_cross_wave_edges_and_bounds_resets() {
    // 1,050 files crosses PASS2_WAVE_FILES (1,000) into 2 coordinated waves.
    // Wave 1 (last file `m1049.py`) calls a blocking helper in Wave 0 (`m0000.py`).
    let mut owned: Vec<(String, String)> = Vec::with_capacity(1050);
    owned.push((
        "m0000.py".to_string(),
        "import time\ndef sync_helper():\n    time.sleep(0.1)\n".to_string(),
    ));
    for i in 1..1049 {
        let name = format!("m{i:04}.py");
        let body = format!("async def entry_{i}():\n    return {i}\n");
        owned.push((name, body));
    }
    owned.push((
        "m1049.py".to_string(),
        "from m0000 import sync_helper\nasync def late_caller():\n    sync_helper()\n".to_string(),
    ));
    let files: Vec<(&str, &str)> = owned
        .iter()
        .map(|(n, b)| (n.as_str(), b.as_str()))
        .collect();
    let dir = temp_project(&files);
    let _ = fs::remove_dir_all(dir.join(".venv"));

    let (report, resets) = scan_path_counting_fresh_empty(&dir).expect("scan");
    assert!(
        report.complete,
        "scan should complete: skipped={:?}",
        report
    );
    assert_eq!(
        report.findings.len(),
        1,
        "cross-wave call from m1049.py to m0000.py must produce 1 finding"
    );
    assert_eq!(report.findings[0].entry.name, "late_caller");
    // 2 coordinated waves = 2 wave resets (never per-worker × N workers).
    assert!(
        (2..=4).contains(&resets),
        "expected 2 coordinated wave resets for 1,050 files, got {resets}"
    );
}
