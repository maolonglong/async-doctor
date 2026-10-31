//! Whole-scan report cache: warm hit, invalidation, stale schema.

use std::fs;
use std::time::Instant;

use super::helpers::{clone_fixture, fresh_cache_dir, report_json_with_cache};

#[test]
fn scan_cache_warm_is_much_faster_when_unchanged() {
    let dir = clone_fixture("m2_sitepkgs_cache");
    let cache = fresh_cache_dir("scan-warm-fast");

    let t0 = Instant::now();
    let cold = report_json_with_cache(&dir, &cache);
    let cold_ms = t0.elapsed();

    let t1 = Instant::now();
    let warm = report_json_with_cache(&dir, &cache);
    let warm_ms = t1.elapsed();

    assert_eq!(cold, warm, "scan-cache warm must be byte-identical to cold");

    // Whole-scan hit should dominate: warm << cold whenever cold did real work.
    if cold_ms.as_millis() >= 200 {
        assert!(
            warm_ms.as_secs_f64() * 3.0 < cold_ms.as_secs_f64(),
            "warm ({warm_ms:?}) should be substantially faster than cold ({cold_ms:?})"
        );
    }

    // Scan cache file must exist under v{schema}/{tool}/scan/.
    let scan_dir = cache
        .join(format!("v{}", async_doctor_ty_cache_schema()))
        .join(env!("CARGO_PKG_VERSION"))
        .join("scan");
    assert!(
        scan_dir.is_dir(),
        "expected scan cache dir at {}",
        scan_dir.display()
    );
    let jsons: Vec<_> = fs::read_dir(&scan_dir)
        .unwrap()
        .flatten()
        .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("json"))
        .collect();
    assert!(
        !jsons.is_empty(),
        "expected at least one scan/*.json under {}",
        scan_dir.display()
    );
}

fn async_doctor_ty_cache_schema() -> u32 {
    // Keep in sync with crate::cache::CACHE_SCHEMA_VERSION (not re-exported).
    13
}

#[test]
fn scan_cache_invalidates_when_source_changes() {
    let dir = clone_fixture("m2_sitepkgs_cache");
    let cache = fresh_cache_dir("scan-invalidate");

    let cold = report_json_with_cache(&dir, &cache);
    let warm = report_json_with_cache(&dir, &cache);
    assert_eq!(cold, warm);

    let app = dir.join("app.py");
    let mut text = fs::read_to_string(&app).unwrap();
    text.push_str("\n# async-doctor fingerprint bump\n");
    fs::write(&app, &text).unwrap();

    let after = report_json_with_cache(&dir, &cache);
    let after2 = report_json_with_cache(&dir, &cache);
    assert_eq!(
        after, after2,
        "after invalidation, warm must match the new cold"
    );
}
