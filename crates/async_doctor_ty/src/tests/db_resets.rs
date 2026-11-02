//! Regression: default parallel extract must not periodically `fresh_empty()`.
//!
//! Periodic rebuilds allocate independent `Arc<Zalsa>` arenas; retaining them
//! until scan end caused OOM on large monorepos (see
//! `docs/notes/2026-11-salsa-fresh-empty-rss.md`).

use std::fs;

use crate::db::AsyncDoctorDb;
use crate::scan::scan_path;

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

    AsyncDoctorDb::reset_fresh_empty_count();
    let report = scan_path(&dir).expect("scan");
    let resets = AsyncDoctorDb::fresh_empty_count();

    assert!(report.complete, "scan should complete: skipped={:?}", report);
    // One post-pass-1 rebuild is expected; allow a small cushion for panic-free
    // follow that does nothing. The old intervals would exceed ~400/160 ≈ 2
    // per worker × N workers ≫ 8.
    assert!(
        resets <= 4,
        "default path must not periodically fresh_empty (got {resets} resets for 400 files)"
    );
}
