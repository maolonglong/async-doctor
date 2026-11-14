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

#[cfg(unix)]
#[test]
fn interpreter_symlink_preserves_selected_venv_across_scan_paths() {
    let tmp = tempfile::tempdir().unwrap();
    let project = tmp.path().join("project");
    let venv = tmp.path().join("selected");
    let base = tmp.path().join("base");
    super::helpers::copy_dir(&advice_shared_venv(), &venv);
    fs::create_dir_all(&project).unwrap();
    fs::create_dir_all(venv.join("bin")).unwrap();
    fs::create_dir_all(base.join("bin")).unwrap();
    fs::create_dir_all(base.join("lib/python3.12/site-packages")).unwrap();
    fs::write(base.join("bin/python"), "must never be executed\n").unwrap();
    let python = venv.join("bin/python");
    std::os::unix::fs::symlink(base.join("bin/python"), &python).unwrap();
    fs::write(
        project.join("app.py"),
        "import asyncio\nimport httpx\nimport requests\n\n\
         async def run():\n    \
         requests.get('https://example.com')\n    \
         await asyncio.to_thread(httpx.get, 'https://example.com')\n",
    )
    .unwrap();
    let empty = tmp.path().join("empty");
    fs::create_dir_all(&empty).unwrap();
    let options = ScanOptions {
        python: Some(python.clone()),
        cache_dir: Some(tmp.path().join("cache")),
        ..ScanOptions::default()
    };
    let expected_venv = dunce::canonicalize(&venv).unwrap();
    let expected_python = expected_venv.join("bin/python");
    for target in [&project, &project.join("app.py"), &empty] {
        let cold = scan_path_with_scan_options(target, &options).unwrap();
        let warm = scan_path_with_scan_options(target, &options).unwrap();
        assert_eq!(cold.environment.origin, "--python");
        assert_eq!(
            cold.environment.prefix.as_deref(),
            expected_venv.to_str(),
            "selected interpreter lost its venv for {}",
            target.display()
        );
        assert_eq!(
            cold.environment.interpreter.as_deref(),
            expected_python.to_str()
        );
        assert_eq!(
            serde_json::to_value(&cold).unwrap(),
            serde_json::to_value(&warm).unwrap()
        );
        if target != &empty {
            assert_eq!(cold.findings.len(), 1);
            assert_eq!(cold.findings[0].rule, "blocking-http");
            assert_eq!(cold.environment.unresolved_third_party, 0);
            assert!(
                cold.advice
                    .iter()
                    .any(|a| a.rule == "offload-has-async-twin")
            );
        }
    }

    // An ordinary interpreter alias without pyvenv.cfg must still follow its target.
    let alias = tmp.path().join("python-alias");
    std::os::unix::fs::symlink(base.join("bin/python"), &alias).unwrap();
    let options = ScanOptions {
        python: Some(alias),
        ..options
    };
    for _ in 0..2 {
        let report = scan_path_with_scan_options(&empty, &options).unwrap();
        assert_eq!(
            report.environment.prefix.as_deref(),
            dunce::canonicalize(&base).unwrap().to_str()
        );
    }
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
