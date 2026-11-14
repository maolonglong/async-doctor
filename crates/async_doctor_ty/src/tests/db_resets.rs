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
    // PASS2_WAVE_FILES + 50 files crosses into 2 coordinated waves. Wave 1
    // (last file) calls a blocking helper in Wave 0 (`m0000.py`).
    let total = crate::extract::pass2::PASS2_WAVE_FILES + 50;
    let last = total - 1;
    let mut owned: Vec<(String, String)> = Vec::with_capacity(total);
    owned.push((
        "m0000.py".to_string(),
        "import time\ndef sync_helper():\n    time.sleep(0.1)\n".to_string(),
    ));
    for i in 1..last {
        let name = format!("m{i:04}.py");
        let body = format!("async def entry_{i}():\n    return {i}\n");
        owned.push((name, body));
    }
    owned.push((
        format!("m{last:04}.py"),
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
        "cross-wave call from the last file to m0000.py must produce 1 finding"
    );
    assert_eq!(report.findings[0].entry.name, "late_caller");
    // 2 coordinated waves = 2 wave resets (never per-worker × N workers).
    assert!(
        (2..=4).contains(&resets),
        "expected 2 coordinated wave resets for {total} files, got {resets}"
    );
}

#[test]
fn clear_parsed_modules_drains_read_paths_incrementally() {
    use ruff_db::Db as SourceDb;
    use ruff_db::files::system_path_to_file;
    use ruff_db::source::source_text;
    use ruff_db::system::SystemPathBuf;

    let dir = temp_project(&[
        ("a.py", "async def fa():\n    return 1\n"),
        ("b.py", "async def fb():\n    return 2\n"),
    ]);
    let root = SystemPathBuf::from_path_buf(dir.clone()).unwrap();
    let db = crate::AsyncDoctorDb::open(&root, None).unwrap();
    let _ = db.system();
    db.clear_parsed_modules_only();
    assert_eq!(
        db.pending_read_paths_count(),
        0,
        "initial clear must drain any startup read_paths"
    );

    let fa = system_path_to_file(&db, root.join("a.py")).unwrap();
    let _ = source_text(&db, fa);
    assert_eq!(db.pending_read_paths_count(), 1);

    db.clear_parsed_modules_only();
    assert_eq!(
        db.pending_read_paths_count(),
        0,
        "clear_parsed_modules_only must delta-drain read_paths instead of retaining O(W^2) set"
    );

    let fb = system_path_to_file(&db, root.join("b.py")).unwrap();
    let _ = source_text(&db, fb);
    assert_eq!(
        db.pending_read_paths_count(),
        1,
        "subsequent reads must record only the newly read file"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn batched_serial_pass2_matches_parallel_and_isolates_panics() {
    let dir = temp_project(&[
        (
            "a_helper.py",
            concat!(
                "import time\n",
                "def run_cb(cb):\n    cb()\n",
                "def sync_leaf():\n    time.sleep(0.1)\n",
            ),
        ),
        (
            "b_panic/__async_doctor_panic__.py",
            "from a_helper import run_cb, sync_leaf\nasync def doomed():\n    run_cb(sync_leaf)\n",
        ),
        (
            "c_ok.py",
            "from a_helper import run_cb, sync_leaf\nasync def healthy():\n    run_cb(sync_leaf)\n",
        ),
    ]);

    let parallel = crate::hermetic_env::with_clean_venv_env(|| crate::scan_path(&dir).unwrap());
    let batched = crate::hermetic_env::with_clean_venv_env(|| {
        unsafe { std::env::set_var("ASYNC_DOCTOR_PASS2_BATCH", "1") };
        let res = crate::scan_path(&dir);
        unsafe { std::env::remove_var("ASYNC_DOCTOR_PASS2_BATCH") };
        res.unwrap()
    });

    assert!(!parallel.complete);
    assert!(!batched.complete);
    assert_eq!(batched.skipped, parallel.skipped);
    assert_eq!(batched.findings, parallel.findings);
    assert!(
        batched
            .findings
            .iter()
            .any(|f| f.rule == "blocking-sleep" && f.entry.name == "healthy"),
        "healthy file after panic must still report findings: {:?}",
        batched.findings
    );
    let _ = fs::remove_dir_all(&dir);
}
