//! M3c recall: nested editable packages + self.engine SQLAlchemy.

use std::fs;

use async_doctor_core::AnalyzeOptions;

use super::helpers::{clone_fixture, fresh_cache_dir, report_json_with_cache, temp_project};
use crate::{scan_path, scan_path_with_cache, scan_path_with_options};

fn assert_four_http_sleep(report: &async_doctor_core::Report, label: &str) {
    assert_eq!(
        report.findings.len(),
        4,
        "{label}: expected 4 findings, got {}: {:?}",
        report.findings.len(),
        report.findings
    );
    let http = report
        .findings
        .iter()
        .filter(|f| f.rule == "blocking-http")
        .count();
    let sleep = report
        .findings
        .iter()
        .filter(|f| f.rule == "blocking-sleep")
        .count();
    assert_eq!(
        http, 2,
        "{label}: expected 2 blocking-http: {:?}",
        report.findings
    );
    assert_eq!(
        sleep, 2,
        "{label}: expected 2 blocking-sleep: {:?}",
        report.findings
    );
}

#[test]
fn m3c_flat_layout_reports_four() {
    let dir = clone_fixture("m3c_nested_pkg_flat");
    let report = scan_path(&dir).unwrap();
    assert_four_http_sleep(&report, "flat");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn m3c_nested_editable_matches_flat() {
    let flat = clone_fixture("m3c_nested_pkg_flat");
    let nested = clone_fixture("m3c_nested_pkg_editable");
    let flat_r = scan_path(&flat).unwrap();
    let nested_r = scan_path(&nested).unwrap();
    assert_four_http_sleep(&flat_r, "flat");
    assert_four_http_sleep(&nested_r, "nested+editable");
    let _ = fs::remove_dir_all(&flat);
    let _ = fs::remove_dir_all(&nested);
}

#[test]
fn m3c_nested_editable_cold_warm_identical() {
    let dir = clone_fixture("m3c_nested_pkg_editable");
    let cache = fresh_cache_dir("m3c-nested");
    let cold = report_json_with_cache(&dir, &cache);
    let warm = report_json_with_cache(&dir, &cache);
    assert_eq!(cold, warm, "cold and warm reports must be byte-identical");
    let report: async_doctor_core::Report = serde_json::from_str(&cold).unwrap();
    assert_four_http_sleep(&report, "nested cold");
    let _ = fs::remove_dir_all(&dir);
    let _ = fs::remove_dir_all(&cache);
}

#[test]
fn m3c_unresolved_import_surfaces_as_candidate() {
    // Missing sibling package: call must not be silently dropped.
    let dir = temp_project(&[(
        "main.py",
        "from missing_pkg.cfg import load_config\n\nasync def handler():\n    return load_config()\n",
    )]);
    let hidden = scan_path(&dir).unwrap();
    assert!(
        hidden.findings.is_empty(),
        "unresolved must not be a finding: {:?}",
        hidden.findings
    );
    assert!(
        hidden.hidden_unresolved_candidates >= 1,
        "expected hidden unresolved candidate, got {}",
        hidden.hidden_unresolved_candidates
    );

    let shown = scan_path_with_options(
        &dir,
        AnalyzeOptions {
            include_non_production: false,
            show_unresolved: true,
        },
    )
    .unwrap();
    assert!(
        shown
            .candidates
            .iter()
            .any(|c| c.rule == "unresolved-call" && c.entry.name == "handler"),
        "expected unresolved candidate for load_config(): {:?}",
        shown.candidates
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn m3c_self_engine_begin_and_connect() {
    let dir = clone_fixture("m3c_self_engine");
    let cache = fresh_cache_dir("m3c-engine");
    let report = scan_path_with_cache(&dir, Some(&cache)).unwrap();
    let names: Vec<_> = report
        .findings
        .iter()
        .map(|f| f.entry.name.as_str())
        .collect();
    assert!(
        names.contains(&"SyncWorker.self_begin"),
        "self.engine.begin() must fire: {names:?}"
    );
    assert!(
        names.contains(&"SyncWorker.self_connect"),
        "self.engine.connect() must fire: {names:?}"
    );
    assert!(
        !names.iter().any(|n| n.contains("async_")),
        "AsyncEngine paths must stay clean: {names:?}"
    );
    assert!(
        report.findings.iter().all(|f| f.rule == "blocking-db"),
        "expected only blocking-db: {:?}",
        report.findings
    );

    let cold = report_json_with_cache(&dir, &cache);
    let warm = report_json_with_cache(&dir, &cache);
    assert_eq!(cold, warm);

    let _ = fs::remove_dir_all(&dir);
    let _ = fs::remove_dir_all(&cache);
}

#[test]
fn m3c_editable_finder_alone_resolves_without_config() {
    // No pyright/ruff/ty config — only the setuptools finder MAPPING.
    let dir = temp_project(&[
        (
            "main.py",
            "from libcore.cfg import load_config\n\nasync def handler():\n    return load_config()\n\nasync def other():\n    return load_config()\n",
        ),
        ("libcore/libcore/__init__.py", ""),
        (
            "libcore/libcore/cfg.py",
            "import time\nimport requests\n\ndef _fetch(key):\n    for attempt in range(3):\n        try:\n            return requests.get(f'https://config.example.com/{key}', timeout=5).text\n        except Exception:\n            time.sleep(0.5 * 2 ** attempt)\n    return ''\n\ndef load_config():\n    return {'secret': _fetch('secrets')}\n",
        ),
        (
            ".venv/pyvenv.cfg",
            "home = /usr/bin\ninclude-system-site-packages = false\nversion = 3.12.0\n",
        ),
        (
            ".venv/lib/python3.12/site-packages/requests/__init__.py",
            "def get(url, **kwargs):\n    class R:\n        text = ''\n    return R()\n",
        ),
        (
            ".venv/lib/python3.12/site-packages/requests-2.0.0.dist-info/METADATA",
            "Name: requests\nVersion: 2.0.0\n",
        ),
        (
            ".venv/lib/python3.12/site-packages/__editable___libcore_0_1_0_finder.py",
            "MAPPING = {'libcore': 'libcore/libcore'}\nNAMESPACES = {}\n",
        ),
        (
            ".venv/lib/python3.12/site-packages/__editable__.libcore-0.1.0.pth",
            "import __editable___libcore_0_1_0_finder\n",
        ),
    ]);
    let report = scan_path(&dir).unwrap();
    assert_four_http_sleep(&report, "finder-only");
    let _ = fs::remove_dir_all(&dir);
}
