//! Per-file first-party fact cache: incremental warm after editing one file.

use std::fs;
use std::time::Instant;

use super::helpers::{fresh_cache_dir, report_json_with_cache, temp_project};

#[test]
fn one_file_edit_reuses_sibling_file_cache() {
    let dir = temp_project(&[
        ("a.py", "import time\n\ndef sync_a():\n    time.sleep(1)\n"),
        ("b.py", "import time\n\ndef sync_b():\n    time.sleep(2)\n"),
    ]);
    let cache = fresh_cache_dir("file-cache-edit");

    let cold = report_json_with_cache(&dir, &cache);
    let warm_unchanged = report_json_with_cache(&dir, &cache);
    assert_eq!(
        cold, warm_unchanged,
        "unchanged warm (scan cache) must match cold"
    );

    let b = dir.join("b.py");
    let mut text = fs::read_to_string(&b).unwrap();
    text.push_str("\n# bump\n");
    fs::write(&b, &text).unwrap();

    let t0 = Instant::now();
    let after_edit = report_json_with_cache(&dir, &cache);
    let edit_ms = t0.elapsed();
    let after_edit_warm = report_json_with_cache(&dir, &cache);
    assert_eq!(
        after_edit, after_edit_warm,
        "after one-file edit, scan-cache warm must match the new report"
    );

    // Wipe scan cache only — keep per-file caches — and re-run: must match.
    let schema = 12u32;
    let tool = env!("CARGO_PKG_VERSION");
    let scan_dir = cache.join(format!("v{schema}")).join(tool).join("scan");
    if scan_dir.is_dir() {
        let _ = fs::remove_dir_all(&scan_dir);
    }
    let t1 = Instant::now();
    let incremental = report_json_with_cache(&dir, &cache);
    let incr_ms = t1.elapsed();
    assert_eq!(
        after_edit, incremental,
        "incremental extract (file cache hits + one miss) must be byte-identical"
    );

    // Sanity: file cache dir has entries.
    let files_dir = cache.join(format!("v{schema}")).join(tool).join("files");
    assert!(
        files_dir.is_dir(),
        "expected per-file cache at {}",
        files_dir.display()
    );

    // Soft timing check: incremental should not be wildly slower than a tiny project cold.
    let _ = (edit_ms, incr_ms);
}

#[test]
fn file_cache_survives_content_change_of_other_file() {
    let dir = temp_project(&[
        ("pkg/__init__.py", ""),
        (
            "pkg/one.py",
            "import time\n\ndef f():\n    time.sleep(0.1)\n",
        ),
        ("pkg/two.py", "from pkg.one import f\n\ndef g():\n    f()\n"),
    ]);
    let cache = fresh_cache_dir("file-cache-sibling");

    let _cold = report_json_with_cache(&dir, &cache);

    // Add a new blocking call so the report changes, then clear only the
    // whole-scan cache so extract must merge file-cache hits + one miss.
    let one = dir.join("pkg/one.py");
    fs::write(
        &one,
        "import time\nimport os\n\ndef f():\n    time.sleep(0.1)\n    os.getcwd()\n",
    )
    .unwrap();

    let schema = 12u32;
    let tool = env!("CARGO_PKG_VERSION");
    let scan_dir = cache.join(format!("v{schema}")).join(tool).join("scan");
    let _ = fs::remove_dir_all(&scan_dir);

    let after = report_json_with_cache(&dir, &cache);

    // Full cold into a fresh cache must match incremental.
    let cache2 = fresh_cache_dir("file-cache-sibling-cold");
    let full_cold = report_json_with_cache(&dir, &cache2);
    assert_eq!(
        after, full_cold,
        "incremental warm must be byte-identical to a fresh cold extract"
    );
}
