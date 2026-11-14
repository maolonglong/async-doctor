//! Regression tests for `--python` `ScanOptions` threading and side-channel-free
//! `Report::ignore_candidate_files` delivery across interleaved / concurrent scans.

use std::fs;

use async_doctor_core::AnalyzeOptions;

use super::helpers::{advice_shared_venv, fresh_cache_dir, temp_project};
use crate::{ScanOptions, scan_path_with_scan_options};

#[test]
fn scan_options_python_threads_through_db_cache_and_twin_resolve() {
    let proj = temp_project(&[
        (
            "app.py",
            "import asyncio\nimport httpx\nimport requests\n\n\
             async def run():\n    \
             requests.get('https://example.com')\n    \
             await asyncio.to_thread(httpx.get, 'https://example.com')\n",
        ),
        (".venv/pyvenv.cfg", "home = /usr/bin\n"),
        (".venv/lib/python3.12/site-packages/.keep", ""),
    ]);
    let cache = fresh_cache_dir("scan-opts-python");
    let venv = advice_shared_venv();

    let no_py = scan_path_with_scan_options(
        &proj,
        &ScanOptions {
            analyze: AnalyzeOptions::default(),
            python: None,
            cache_dir: Some(cache.clone()),
        },
    )
    .unwrap();
    assert!(no_py.findings.is_empty(), "{:?}", no_py.findings);
    assert_eq!(
        no_py.environment.unresolved_third_party_modules,
        vec!["httpx".to_string(), "requests".to_string()]
    );

    let with_py_cold = scan_path_with_scan_options(
        &proj,
        &ScanOptions {
            analyze: AnalyzeOptions::default(),
            python: Some(venv.clone()),
            cache_dir: Some(cache.clone()),
        },
    )
    .unwrap();
    assert_eq!(with_py_cold.environment.origin, "--python");
    assert_eq!(with_py_cold.findings.len(), 1);
    assert_eq!(with_py_cold.findings[0].rule, "blocking-http");
    assert!(
        with_py_cold
            .advice
            .iter()
            .any(|a| a.rule == "offload-has-async-twin"),
        "twin_resolve must inspect --python site-packages: {:?}",
        with_py_cold.advice
    );

    let with_py_warm = scan_path_with_scan_options(
        &proj,
        &ScanOptions {
            analyze: AnalyzeOptions::default(),
            python: Some(venv),
            cache_dir: Some(cache.clone()),
        },
    )
    .unwrap();
    assert_eq!(
        serde_json::to_string(&with_py_cold).unwrap(),
        serde_json::to_string(&with_py_warm).unwrap(),
        "cold and warm --python JSON must match"
    );

    let _ = fs::remove_dir_all(&proj);
    let _ = fs::remove_dir_all(&cache);
}

#[test]
fn ignore_candidate_files_are_isolated_per_report_across_interleaved_scans() {
    let proj_a = temp_project(&[
        (
            "suppressed.py",
            "# async-doctor: ignore[blocking-sleep]: intentional\nimport time\n\nasync def a():\n    time.sleep(1)\n",
        ),
        ("plain.py", "async def b():\n    return 1\n"),
    ]);
    let proj_b = temp_project(&[("clean.py", "async def c():\n    return 2\n")]);
    let cache = fresh_cache_dir("ignore-cand-iso");

    let opts = ScanOptions {
        analyze: AnalyzeOptions::default(),
        python: None,
        cache_dir: Some(cache.clone()),
    };

    // Interleave: scan A (cold), then scan B (cold) BEFORE inspecting A's ignore candidates.
    // A process-global Mutex side-channel would have overwritten A with B.
    let cold_a = scan_path_with_scan_options(&proj_a, &opts).unwrap();
    let cold_b = scan_path_with_scan_options(&proj_b, &opts).unwrap();

    let cands_a = cold_a
        .ignore_candidate_files
        .as_ref()
        .expect("directory scan populates ignore_candidate_files");
    assert_eq!(cands_a.len(), 1, "{cands_a:?}");
    assert!(cands_a[0].ends_with("suppressed.py"), "{cands_a:?}");

    let cands_b = cold_b
        .ignore_candidate_files
        .as_ref()
        .expect("directory scan populates ignore_candidate_files");
    assert!(cands_b.is_empty(), "{cands_b:?}");

    // Warm hits must also populate identical `ignore_candidate_files`.
    let warm_a = scan_path_with_scan_options(&proj_a, &opts).unwrap();
    let warm_b = scan_path_with_scan_options(&proj_b, &opts).unwrap();
    assert_eq!(warm_a.ignore_candidate_files, cold_a.ignore_candidate_files);
    assert_eq!(warm_b.ignore_candidate_files, cold_b.ignore_candidate_files);

    let _ = fs::remove_dir_all(&proj_a);
    let _ = fs::remove_dir_all(&proj_b);
    let _ = fs::remove_dir_all(&cache);
}
