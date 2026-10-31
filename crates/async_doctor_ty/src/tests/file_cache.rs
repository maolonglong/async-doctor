//! Per-file first-party fact cache: incremental warm must match cold byte-for-byte.

use std::fs;
use std::time::Instant;

use super::helpers::{fresh_cache_dir, report_json_with_cache, temp_project};
use crate::cache::CACHE_SCHEMA_VERSION;

fn wipe_scan_cache(cache: &std::path::Path) {
    let tool = env!("CARGO_PKG_VERSION");
    let scan_dir = cache
        .join(format!("v{CACHE_SCHEMA_VERSION}"))
        .join(tool)
        .join("scan");
    if scan_dir.is_dir() {
        let _ = fs::remove_dir_all(&scan_dir);
    }
}

fn incremental_vs_cold(dir: &std::path::Path, cache: &std::path::Path, label: &str) {
    wipe_scan_cache(cache);
    let incremental = report_json_with_cache(dir, cache);
    let cache2 = fresh_cache_dir(&format!("{label}-cold"));
    let cold = report_json_with_cache(dir, &cache2);
    assert_eq!(
        incremental, cold,
        "{label}: incremental must be byte-identical to cold\nincr={incremental}\ncold={cold}"
    );
    let _ = fs::remove_dir_all(&cache2);
}

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

    wipe_scan_cache(&cache);
    let t1 = Instant::now();
    let incremental = report_json_with_cache(&dir, &cache);
    let incr_ms = t1.elapsed();
    assert_eq!(
        after_edit, incremental,
        "incremental extract (file cache hits + one miss) must be byte-identical"
    );

    let tool = env!("CARGO_PKG_VERSION");
    let files_dir = cache
        .join(format!("v{CACHE_SCHEMA_VERSION}"))
        .join(tool)
        .join("files");
    assert!(
        files_dir.is_dir(),
        "expected per-file cache at {}",
        files_dir.display()
    );

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

    let one = dir.join("pkg/one.py");
    fs::write(
        &one,
        "import time\nimport os\n\ndef f():\n    time.sleep(0.1)\n    os.getcwd()\n",
    )
    .unwrap();

    incremental_vs_cold(&dir, &cache, "sibling-edit");
}

#[test]
fn rename_imported_symbol_matches_cold() {
    // Repro 1: `from pkg import util` then rename util.work → work2.
    let dir = temp_project(&[
        ("pkg/__init__.py", ""),
        (
            "pkg/util.py",
            "import time\n\ndef work():\n    time.sleep(1)\n",
        ),
        (
            "app.py",
            "from pkg import util\n\nasync def handler():\n    util.work()\n",
        ),
    ]);
    let cache = fresh_cache_dir("file-cache-rename");
    let _ = report_json_with_cache(&dir, &cache);

    fs::write(
        dir.join("pkg/util.py"),
        "import time\n\ndef work2():\n    time.sleep(1)\n",
    )
    .unwrap();

    incremental_vs_cold(&dir, &cache, "rename-work");
}

#[test]
fn delete_imported_module_matches_cold() {
    // Repro 4a: delete pkg/util.py
    let dir = temp_project(&[
        ("pkg/__init__.py", ""),
        (
            "pkg/util.py",
            "import time\n\ndef work():\n    time.sleep(1)\n",
        ),
        (
            "app.py",
            "from pkg import util\n\nasync def handler():\n    util.work()\n",
        ),
        (
            "other.py",
            "import time\n\ndef keep():\n    time.sleep(0.1)\n",
        ),
    ]);
    let cache = fresh_cache_dir("file-cache-delete");
    let _ = report_json_with_cache(&dir, &cache);

    fs::remove_file(dir.join("pkg/util.py")).unwrap();

    incremental_vs_cold(&dir, &cache, "delete-util");
}

#[test]
fn delete_reexport_origin_matches_cold() {
    // Repro 4b: app imports mid; mid re-exports from low; delete low.
    let dir = temp_project(&[
        ("pkg/__init__.py", ""),
        (
            "pkg/low.py",
            "import time\n\ndef sink():\n    time.sleep(1)\n",
        ),
        ("pkg/mid.py", "from pkg.low import sink\n"),
        (
            "app.py",
            "from pkg.mid import sink\n\nasync def handler():\n    sink()\n",
        ),
    ]);
    let cache = fresh_cache_dir("file-cache-reexport");
    let _ = report_json_with_cache(&dir, &cache);

    fs::remove_file(dir.join("pkg/low.py")).unwrap();

    incremental_vs_cold(&dir, &cache, "delete-reexport-origin");
}

#[test]
fn package_dir_root_offset_shift_matches_cold() {
    // Repro 2+3: scan root = package dir; absolute imports use pkg.*; prepend
    // a def so name_starts shift in the callee file.
    let dir = temp_project(&[
        ("__init__.py", ""),
        (
            "util.py",
            "import time\n\ndef work():\n    time.sleep(1)\n",
        ),
        (
            "app.py",
            "from pkg.util import work\n\nasync def handler():\n    work()\n",
        ),
    ]);
    // Directory name must be `pkg` for package-root aliases (scan root = pkg/).
    let parent = dir.parent().unwrap().join(format!(
        "async-doctor-pkg-root-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let pkg = parent.join("pkg");
    let _ = fs::remove_dir_all(&parent);
    fs::create_dir_all(&pkg).unwrap();
    for name in ["__init__.py", "util.py", "app.py"] {
        fs::copy(dir.join(name), pkg.join(name)).unwrap();
    }

    let cache = fresh_cache_dir("file-cache-pkg-root");
    let _ = report_json_with_cache(&pkg, &cache);

    // Offset shift: insert a function above `work`.
    fs::write(
        pkg.join("util.py"),
        "import time\n\ndef _pad():\n    pass\n\ndef work():\n    time.sleep(1)\n",
    )
    .unwrap();

    incremental_vs_cold(&pkg, &cache, "pkg-root-offset");
    let _ = fs::remove_dir_all(&parent);
}

#[test]
fn same_second_same_size_edit_invalidates_scan_cache() {
    // Repro 5: same-size same-second edit must not return a stale whole-scan hit.
    let dir = temp_project(&[(
        "a.py",
        "import time\n\nasync def h():\n    time.sleep(1)\n",
    )]);
    let cache = fresh_cache_dir("file-cache-same-sec");
    let cold = report_json_with_cache(&dir, &cache);
    let cold_r: async_doctor_core::Report = serde_json::from_str(&cold).unwrap();
    assert_eq!(cold_r.findings.len(), 1);

    // Same length: sleep → sleeq
    let py = dir.join("a.py");
    let text = fs::read_to_string(&py).unwrap().replace("sleep", "sleeq");
    assert_eq!(
        text.len(),
        fs::read_to_string(&py).unwrap().len(),
        "edit must be same size"
    );
    fs::write(&py, &text).unwrap();

    let after = report_json_with_cache(&dir, &cache);
    let after_r: async_doctor_core::Report = serde_json::from_str(&after).unwrap();
    assert_eq!(
        after_r.findings.len(),
        0,
        "same-second same-size edit must invalidate scan cache, got {after}"
    );

    let cache2 = fresh_cache_dir("file-cache-same-sec-cold");
    let cold2 = report_json_with_cache(&dir, &cache2);
    assert_eq!(after, cold2, "warm after same-size edit must match cold");
    let _ = fs::remove_dir_all(&cache2);
}

#[test]
fn stale_file_cache_schema_is_not_reused() {
    let dir = temp_project(&[(
        "a.py",
        "import time\n\ndef sync_a():\n    time.sleep(1)\n",
    )]);
    let cache = fresh_cache_dir("file-cache-stale-schema");
    let _ = report_json_with_cache(&dir, &cache);

    let tool = env!("CARGO_PKG_VERSION");
    let files_root = cache
        .join(format!("v{CACHE_SCHEMA_VERSION}"))
        .join(tool)
        .join("files");
    assert!(files_root.is_dir());
    // Spoof every per-file JSON to a prior schema.
    for entry in walkdir_json(&files_root) {
        let data = fs::read_to_string(&entry).unwrap();
        let mut spoofed: serde_json::Value = serde_json::from_str(&data).unwrap();
        if spoofed.get("schema_version").is_some() {
            spoofed["schema_version"] = serde_json::json!(CACHE_SCHEMA_VERSION - 1);
            fs::write(&entry, serde_json::to_string(&spoofed).unwrap()).unwrap();
        }
    }
    wipe_scan_cache(&cache);
    // Must re-extract (stale schema ignored) and still match a fresh cold.
    let after = report_json_with_cache(&dir, &cache);
    let cache2 = fresh_cache_dir("file-cache-stale-cold");
    let cold = report_json_with_cache(&dir, &cache2);
    assert_eq!(after, cold, "stale schema slots must not be reused");
    let _ = fs::remove_dir_all(&cache2);
}

fn walkdir_json(root: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = fs::read_dir(&dir) else {
            continue;
        };
        for ent in rd.flatten() {
            let p = ent.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().and_then(|e| e.to_str()) == Some("json") {
                out.push(p);
            }
        }
    }
    out
}
