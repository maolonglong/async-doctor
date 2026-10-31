//! Whole-scan report cache: warm hit, invalidation, stale schema.

use std::fs;
use std::time::Instant;

use super::helpers::{clone_fixture, fresh_cache_dir, report_json_with_cache, temp_project};

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
    crate::cache::CACHE_SCHEMA_VERSION
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

#[test]
fn scan_cache_invalidates_on_pyi_add_and_edit() {
    // First-party .pyi stubs affect ty resolution but are not extract inputs;
    // the whole-scan fingerprint must still include them.
    let dir = temp_project(&[
        ("pkg/__init__.py", ""),
        (
            "pkg/util.py",
            concat!(
                "class Client:\n",
                "    def block(self):\n",
                "        import time\n",
                "        time.sleep(1)\n",
                "\n",
                "def get_client():\n",
                "    return Client()\n",
            ),
        ),
        (
            "app.py",
            "from pkg.util import get_client\n\nasync def handler():\n    get_client().block()\n",
        ),
    ]);
    let cache = fresh_cache_dir("scan-pyi");

    let cold0 = report_json_with_cache(&dir, &cache);
    let warm0 = report_json_with_cache(&dir, &cache);
    assert_eq!(cold0, warm0);
    let cold0_r: async_doctor_core::Report = serde_json::from_str(&cold0).unwrap();
    assert!(
        cold0_r
            .findings
            .iter()
            .any(|f| f.rule == "blocking-sleep" && f.entry.name == "handler"),
        "baseline should report blocking-sleep: {:?}",
        cold0_r
            .findings
            .iter()
            .map(|f| (&f.rule, &f.entry.name))
            .collect::<Vec<_>>()
    );

    // Add a stub that types get_client() as object so .block() no longer
    // resolves to Client.block → cold findings change; warm must miss.
    fs::write(
        dir.join("pkg/util.pyi"),
        "def get_client() -> object: ...\n",
    )
    .unwrap();

    let after_add = report_json_with_cache(&dir, &cache);
    assert_ne!(
        after_add, cold0,
        "adding util.pyi must change graded output (else warm could falsely hit)"
    );
    let cache_cold = fresh_cache_dir("scan-pyi-cold-add");
    let fresh_cold_add = report_json_with_cache(&dir, &cache_cold);
    assert_eq!(
        after_add, fresh_cold_add,
        "after adding .pyi, cached scan must equal a fresh cold"
    );
    let _ = fs::remove_dir_all(&cache_cold);

    // Edit the stub again — must miss and match cold.
    fs::write(
        dir.join("pkg/util.pyi"),
        "from typing import Any\ndef get_client() -> Any: ...\n",
    )
    .unwrap();
    let after_edit = report_json_with_cache(&dir, &cache);
    let cache_cold2 = fresh_cache_dir("scan-pyi-cold-edit");
    let fresh_cold_edit = report_json_with_cache(&dir, &cache_cold2);
    assert_eq!(
        after_edit, fresh_cold_edit,
        "after editing .pyi, cached scan must equal a fresh cold"
    );
    let _ = fs::remove_dir_all(&cache_cold2);
}
