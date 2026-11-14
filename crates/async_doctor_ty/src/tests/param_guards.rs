//! Regression tests for multi-guard `ParamGuard` conjunction, `else` / early-return
//! guard inversion, `keyword_literals`, and `PackageCache` cold/warm parity.

use std::collections::BTreeSet;
use std::fs;

use super::helpers::{fresh_cache_dir, report_json_with_cache, temp_project};
use crate::scan_path;

#[test]
fn nested_and_boolean_and_param_guards_never_overwrite_outer_guard() {
    let dir = temp_project(&[(
        "app.py",
        "import time\n\n\
         def nested(sync: bool, verbose: bool) -> None:\n    \
             if sync:\n        \
                 if verbose:\n            \
                     time.sleep(1)\n\n\
         def conjunctive(sync: bool, verbose: bool) -> None:\n    \
             if sync and verbose:\n        \
                 time.sleep(1)\n\n\
         async def clean_false_outer():\n    \
             nested(False, True)\n    \
             conjunctive(False, True)\n\n\
         async def clean_false_inner():\n    \
             nested(True, False)\n    \
             conjunctive(True, False)\n\n\
         async def blocks_when_both_true():\n    \
             nested(True, True)\n    \
             conjunctive(True, True)\n",
    )]);

    let report = scan_path(&dir).unwrap();
    let finding_entries: BTreeSet<&str> = report
        .findings
        .iter()
        .map(|f| f.entry.name.as_str())
        .collect();
    assert_eq!(
        finding_entries,
        BTreeSet::from(["blocks_when_both_true"]),
        "findings={:?} candidates={:?}",
        report.findings,
        report.candidates
    );
    assert_eq!(report.findings.len(), 2, "{:?}", report.findings);
    assert!(
        report.candidates.is_empty(),
        "f(False, True) and f(True, False) must prune as ProvablyFalse, got candidates: {:?}",
        report.candidates
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn else_branch_and_early_return_invert_param_guards() {
    let dir = temp_project(&[(
        "app.py",
        "import time\n\n\
         def via_else(async_mode: bool) -> int:\n    \
             if async_mode:\n        \
                 return 1\n    \
             else:\n        \
                 time.sleep(1)\n        \
                 return 2\n\n\
         def via_early_return(sync_mode: bool | None) -> int:\n    \
             if not sync_mode:\n        \
                 return 1\n    \
             time.sleep(1)\n    \
             return 2\n\n\
         async def clean_callers():\n    \
             via_else(True)\n    \
             via_early_return(False)\n    \
             via_early_return(None)\n\n\
         async def blocking_else():\n    \
             via_else(False)\n\n\
         async def blocking_early():\n    \
             via_early_return(True)\n",
    )]);

    let report = scan_path(&dir).unwrap();
    let finding_entries: BTreeSet<&str> = report
        .findings
        .iter()
        .map(|f| f.entry.name.as_str())
        .collect();
    assert_eq!(
        finding_entries,
        BTreeSet::from(["blocking_early", "blocking_else"]),
        "findings={:?} candidates={:?}",
        report.findings,
        report.candidates
    );
    assert!(
        report.candidates.is_empty(),
        "provably-false else / early-return calls must not become candidates: {:?}",
        report.candidates
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn keyword_literals_and_multi_guards_preserve_package_cache_cold_warm_parity() {
    let make_tree = || {
        temp_project(&[
            (
                ".venv/pyvenv.cfg",
                "home = /usr\ninclude-system-site-packages = false\nversion = 3.12.3\n",
            ),
            (
                ".venv/lib/python3.12/site-packages/guardpkg-1.0.0.dist-info/METADATA",
                "Metadata-Version: 2.1\nName: guardpkg\nVersion: 1.0.0\n",
            ),
            (
                ".venv/lib/python3.12/site-packages/guardpkg-1.0.0.dist-info/RECORD",
                "guardpkg/__init__.py,,\nguardpkg/api.py,,\nguardpkg-1.0.0.dist-info/METADATA,,\n",
            ),
            (
                ".venv/lib/python3.12/site-packages/guardpkg/__init__.py",
                "import time\n\n\
                 def _sleep_now() -> None:\n    \
                     time.sleep(1)\n\n\
                 def fetch(url: str, *, sync_mode: bool = False, verbose: bool = False) -> None:\n    \
                     if sync_mode:\n        \
                         if verbose:\n            \
                             _sleep_now()\n\n\
                 def run_fallback(use_async: bool = True) -> str:\n    \
                     if use_async:\n        \
                         return 'ok'\n    \
                     else:\n        \
                         time.sleep(1)\n        \
                         return 'sync'\n",
            ),
            (
                ".venv/lib/python3.12/site-packages/guardpkg/api.py",
                "from . import fetch, run_fallback\n\n\
                 def pkg_kw_clean() -> None:\n    \
                     fetch('https://example.com', sync_mode=False, verbose=True)\n    \
                     run_fallback(use_async=True)\n\n\
                 def pkg_kw_blocks() -> None:\n    \
                     fetch('https://example.com', sync_mode=True, verbose=True)\n",
            ),
            (
                "app.py",
                "from guardpkg import fetch, run_fallback\n\
                 from guardpkg.api import pkg_kw_blocks, pkg_kw_clean\n\n\
                 async def kw_clean():\n    \
                     fetch('https://example.com', sync_mode=False, verbose=True)\n    \
                     run_fallback(use_async=True)\n    \
                     pkg_kw_clean()\n\n\
                 async def kw_blocks_nested():\n    \
                     fetch('https://example.com', sync_mode=True, verbose=True)\n\n\
                 async def kw_blocks_via_pkg_edge():\n    \
                     pkg_kw_blocks()\n\n\
                 async def kw_else_candidate():\n    \
                     run_fallback(use_async=False)\n",
            ),
        ])
    };

    // Tree 1 populates `PackageCache` for `guardpkg` 1.0.0.
    // Tree 2 has a distinct scan_root (missing whole-scan cache) so it materializes
    // `guardpkg` from the on-disk `PackageCache` (exercising both `CachedEdge.param_guards`
    // and `CachedEdge.keyword_literals`).
    let tree_cold = make_tree();
    let tree_warm_pkg = make_tree();
    let cache = fresh_cache_dir("pkg-cache-param-guards");

    let json_cold = report_json_with_cache(&tree_cold, &cache);
    let json_warm_pkg = report_json_with_cache(&tree_warm_pkg, &cache);

    let mut cold: serde_json::Value = serde_json::from_str(&json_cold).unwrap();
    let mut warm: serde_json::Value = serde_json::from_str(&json_warm_pkg).unwrap();
    // Normalize scan-root-dependent environment paths before comparing full report JSON.
    cold["environment"]
        .as_object_mut()
        .unwrap()
        .remove("prefix");
    cold["environment"]
        .as_object_mut()
        .unwrap()
        .remove("sitePackages");
    warm["environment"]
        .as_object_mut()
        .unwrap()
        .remove("prefix");
    warm["environment"]
        .as_object_mut()
        .unwrap()
        .remove("sitePackages");
    assert_eq!(
        cold, warm,
        "PackageCache warm materialization must match cold extract bit-for-bit"
    );

    let finding_entries: BTreeSet<&str> = cold["findings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["entry"]["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        finding_entries,
        BTreeSet::from(["kw_blocks_nested", "kw_blocks_via_pkg_edge"]),
        "findings={}",
        cold["findings"]
    );
    // Installed third-party direct branch sink (`run_fallback` else) is a candidate when
    // `use_async=False`, while `use_async=True` in `kw_clean` is pruned as ProvablyFalse.
    let candidate_entries: BTreeSet<&str> = cold["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["entry"]["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        candidate_entries,
        BTreeSet::from(["kw_else_candidate"]),
        "candidates={}",
        cold["candidates"]
    );

    let _ = fs::remove_dir_all(&tree_cold);
    let _ = fs::remove_dir_all(&tree_warm_pkg);
    let _ = fs::remove_dir_all(&cache);
}
