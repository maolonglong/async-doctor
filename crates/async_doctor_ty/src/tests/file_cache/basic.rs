//! Non-scen incremental file-cache regressions (sibling reuse, schema, deps_fp).

use std::fs;
use std::time::Instant;

use super::super::helpers::{
    clone_fixture, fresh_cache_dir, report_json_with_cache, temp_project, with_cleared_virtual_env,
};
use super::helpers::{
    files_cache_dir, incremental_vs_cold, read_slot_json, walkdir_json, wipe_scan_cache,
};
use crate::cache::CACHE_SCHEMA_VERSION;

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

    assert!(
        files_cache_dir(&cache).is_dir(),
        "expected per-file cache at {}",
        files_cache_dir(&cache).display()
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
fn same_second_same_size_edit_invalidates_scan_cache() {
    let dir = temp_project(&[("a.py", "import time\n\nasync def h():\n    time.sleep(1)\n")]);
    let cache = fresh_cache_dir("file-cache-same-sec");
    let cold = report_json_with_cache(&dir, &cache);
    let cold_r: async_doctor_core::Report = serde_json::from_str(&cold).unwrap();
    assert_eq!(cold_r.findings.len(), 1);

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
    let dir = temp_project(&[("a.py", "import time\n\ndef sync_a():\n    time.sleep(1)\n")]);
    let cache = fresh_cache_dir("file-cache-stale-schema");
    let _ = report_json_with_cache(&dir, &cache);

    let files_root = files_cache_dir(&cache);
    assert!(files_root.is_dir());
    for entry in walkdir_json(&files_root) {
        if entry.file_name().and_then(|n| n.to_str()) == Some("_project_meta.json") {
            let data = fs::read_to_string(&entry).unwrap();
            let mut spoofed: serde_json::Value = serde_json::from_str(&data).unwrap();
            spoofed["schema_version"] = serde_json::json!(CACHE_SCHEMA_VERSION - 1);
            fs::write(&entry, serde_json::to_string(&spoofed).unwrap()).unwrap();
            continue;
        }
        let data = read_slot_json(&entry);
        let mut spoofed: serde_json::Value = serde_json::from_str(&data).unwrap();
        if spoofed.get("schema_version").is_some() {
            spoofed["schema_version"] = serde_json::json!(CACHE_SCHEMA_VERSION - 1);
            let raw = serde_json::to_vec(&spoofed).unwrap();
            let f = fs::File::create(&entry).unwrap();
            let mut enc = flate2::write::GzEncoder::new(f, flate2::Compression::fast());
            use std::io::Write;
            enc.write_all(&raw).unwrap();
            enc.finish().unwrap();
        }
    }
    wipe_scan_cache(&cache);
    let after = report_json_with_cache(&dir, &cache);
    let cache2 = fresh_cache_dir("file-cache-stale-cold");
    let cold = report_json_with_cache(&dir, &cache2);
    assert_eq!(after, cold, "stale schema slots must not be reused");
    let _ = fs::remove_dir_all(&cache2);
}

#[test]
fn deps_fp_populated_on_cold_save() {
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
    let cache = fresh_cache_dir("file-cache-deps-fp-debug");
    let _ = report_json_with_cache(&dir, &cache);
    let files_root = files_cache_dir(&cache);
    let mut mid_has_deps = false;
    let mut any_deps = false;
    for entry in walkdir_json(&files_root) {
        if entry.file_name().and_then(|n| n.to_str()) == Some("_project_meta.json") {
            continue;
        }
        let data = read_slot_json(&entry);
        let v: serde_json::Value = serde_json::from_str(&data).unwrap();
        let rel = v["rel"].as_str().unwrap_or("");
        let deps = v["deps_fp"].as_str().unwrap_or("");
        if !deps.is_empty() {
            any_deps = true;
        }
        if rel == "pkg/mid.py" && !deps.is_empty() {
            mid_has_deps = true;
        }
    }
    assert!(any_deps, "expected non-empty deps_fp on cold save");
    assert!(
        mid_has_deps,
        "re-export module must store a transitive import deps_fp"
    );
}

#[test]
fn package_dir_root_offset_shift_matches_cold() {
    // Kept alongside scen_absroot: asserts offset-only shift under a package-dir root.
    let dir = temp_project(&[
        ("__init__.py", ""),
        ("util.py", "import time\n\ndef work():\n    time.sleep(1)\n"),
        (
            "app.py",
            "from pkg.util import work\n\nasync def handler():\n    work()\n",
        ),
    ]);
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

    fs::write(
        pkg.join("util.py"),
        "import time\n\ndef _pad():\n    pass\n\ndef work():\n    time.sleep(1)\n",
    )
    .unwrap();

    incremental_vs_cold(&pkg, &cache, "pkg-root-offset");
    let _ = fs::remove_dir_all(&parent);
}

/// Advice-tier fixtures must round-trip `advice[]` + findings through the
/// per-file cache (schema v14 `positional_externals` / loop-guard tags).
#[test]
fn advice_fixtures_incremental_matches_cold() {
    with_cleared_virtual_env(|| {
        for name in [
            "advice_review_d",
            "advice_review_h",
            "advice_review_g",
            "advice_review_a",
            "advice_review_c",
            "advice_offload_twin",
        ] {
            let dir = clone_fixture(name);
            let cache = fresh_cache_dir(&format!("file-cache-{name}"));
            let _ = report_json_with_cache(&dir, &cache);
            // Touch a leaf so scan-cache misses but file slots stay hits where sound.
            let app = dir.join("app.py");
            if app.is_file() {
                let mut text = fs::read_to_string(&app).unwrap();
                text.push_str("\n# incremental bump\n");
                fs::write(&app, &text).unwrap();
            }
            incremental_vs_cold(&dir, &cache, name);
        }
    });
}
