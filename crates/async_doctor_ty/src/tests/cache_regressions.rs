//! Regression tests for Milestone 2 cache soundness and site-packages metadata.

use std::fs;

use async_doctor_core::AnalyzeOptions;

use super::helpers::{clone_fixture, fresh_cache_dir, report_json_with_cache_budget, temp_project};
use crate::extract::FOLLOW_BUDGET_DEFAULT;
use crate::scan_cache::scan_input_fingerprint;
use crate::scan_path_with_cache;
use crate::sitepkgs::PackageMetaMemo;

#[test]
fn cross_package_follow_target_preserved_when_sibling_package_not_loaded() {
    let shared_venv = fresh_cache_dir("shared-venv-site");
    let sp = shared_venv.join("lib/python3.12/site-packages");
    fs::create_dir_all(sp.join("liba")).unwrap();
    fs::create_dir_all(sp.join("liba-1.0.0.dist-info")).unwrap();
    fs::write(
        sp.join("liba/__init__.py"),
        concat!(
            "from libb import sink_helper\n",
            "def entry():\n",
            "    return sink_helper()\n",
            "def other_clean():\n",
            "    return 42\n",
        ),
    )
    .unwrap();
    fs::write(
        sp.join("liba-1.0.0.dist-info/METADATA"),
        "Name: liba\nVersion: 1.0.0\n",
    )
    .unwrap();
    fs::write(
        sp.join("liba-1.0.0.dist-info/RECORD"),
        "liba/__init__.py,,\n",
    )
    .unwrap();

    fs::create_dir_all(sp.join("libb")).unwrap();
    fs::create_dir_all(sp.join("libb-1.0.0.dist-info")).unwrap();
    fs::write(
        sp.join("libb/__init__.py"),
        "import time\ndef sink_helper():\n    time.sleep(0.1)\n",
    )
    .unwrap();
    fs::write(
        sp.join("libb-1.0.0.dist-info/METADATA"),
        "Name: libb\nVersion: 1.0.0\n",
    )
    .unwrap();
    fs::write(
        sp.join("libb-1.0.0.dist-info/RECORD"),
        "libb/__init__.py,,\n",
    )
    .unwrap();
    fs::write(shared_venv.join("pyvenv.cfg"), "home = /usr/bin\n").unwrap();

    let proj1 = temp_project(&[(
        "app.py",
        "import liba\nasync def route():\n    liba.entry()\n",
    )]);
    let proj2 = temp_project(&[(
        "app.py",
        "import liba\nasync def route():\n    liba.other_clean()\n",
    )]);
    // Link `.venv` in both projects to the shared venv.
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&shared_venv, proj1.join(".venv")).unwrap();
        std::os::unix::fs::symlink(&shared_venv, proj2.join(".venv")).unwrap();
    }

    let cache = fresh_cache_dir("cross-pkg-preserve");
    // Step 1: proj1 populates liba + libb in PackageCache.
    let r1 = scan_path_with_cache(&proj1, Some(&cache)).unwrap();
    assert_eq!(r1.findings.len(), 1, "proj1 cold must find blocking-sleep");

    // Step 2: proj2 mutates liba's PackageCache (adding other_clean) without loading libb.
    let r2 = scan_path_with_cache(&proj2, Some(&cache)).unwrap();
    assert!(r2.findings.is_empty(), "proj2 should have 0 findings");

    // Step 3: invalidate proj1's whole-scan cache by editing app.py, forcing a warm PackageCache run.
    fs::write(
        proj1.join("app.py"),
        "# invalidate scan cache\nimport liba\nasync def route():\n    liba.entry()\n",
    )
    .unwrap();
    let r3 = scan_path_with_cache(&proj1, Some(&cache)).unwrap();
    assert_eq!(
        r3.findings.len(),
        1,
        "proj1 warm PackageCache must still follow liba.entry -> libb.sink_helper: {:?}",
        r3.findings
    );

    let _ = fs::remove_dir_all(&proj1);
    let _ = fs::remove_dir_all(&proj2);
    let _ = fs::remove_dir_all(&shared_venv);
    let _ = fs::remove_dir_all(&cache);
}

#[test]
fn tiny_budget_seed_does_not_poison_later_full_budget_package_cache() {
    let dir = clone_fixture("m2_follow_budget_truncation");
    let cache = fresh_cache_dir("tiny-then-full");

    let tiny = report_json_with_cache_budget(&dir, &cache, 2);
    let tiny_r: async_doctor_core::Report = serde_json::from_str(&tiny).unwrap();
    assert!(tiny_r.findings.is_empty());

    let full = report_json_with_cache_budget(&dir, &cache, FOLLOW_BUDGET_DEFAULT);
    let full_r: async_doctor_core::Report = serde_json::from_str(&full).unwrap();
    assert!(
        full_r.findings.iter().any(|f| f.rule == "blocking-sleep"),
        "full-budget scan after tiny-budget seed must still find blocking-sleep via libb.gate: {:?}",
        full_r.findings
    );

    let _ = fs::remove_dir_all(&dir);
    let _ = fs::remove_dir_all(&cache);
}

#[test]
fn budget_skipped_does_not_double_count_across_waves() {
    let dir = temp_project(&[(
        "app.py",
        "import liba\nasync def route():\n    liba.entry()\n",
    )]);
    let sp = dir.join(".venv/lib/python3.12/site-packages");
    fs::create_dir_all(sp.join("liba")).unwrap();
    fs::write(dir.join(".venv/pyvenv.cfg"), "home = /usr/bin\n").unwrap();
    fs::write(
        sp.join("liba/__init__.py"),
        concat!(
            "def h1_sub():\n    return 1\n",
            "def h1():\n    return h1_sub()\n",
            "def h2():\n    return 2\n",
            "def h3():\n    return 3\n",
            "def h4():\n    return 4\n",
            "def entry():\n    h1()\n    h1()\n    h2()\n    h3()\n    h4()\n",
        ),
    )
    .unwrap();
    let cache = fresh_cache_dir("budget-dedup");
    // Wave 1 (free): entry()
    // Wave 2 (budget=1): candidates are h1 (2 calls), h2, h3, h4 -> follows h1, skips {h2, h3, h4}
    // Wave 3 (remaining=0): candidates are {h1_sub, h2, h3, h4} -> total unique skipped = 4 (not 3 + 4 = 7!)
    let out = report_json_with_cache_budget(&dir, &cache, 1);
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    let skipped = v["environment"]["followBudgetSkipped"]
        .as_u64()
        .unwrap_or(0);
    assert_eq!(
        skipped, 4,
        "unique budget-skipped third-party functions should be 4 (h1_sub, h2, h3, h4), got {skipped}"
    );

    let _ = fs::remove_dir_all(&dir);
    let _ = fs::remove_dir_all(&cache);
}

#[test]
fn package_cache_preserves_bound_method_param_guard_on_warm_run() {
    let shared_venv = fresh_cache_dir("bound-guard-venv");
    let sp = shared_venv.join("lib/python3.12/site-packages");
    fs::create_dir_all(sp.join("guardedpkg")).unwrap();
    fs::create_dir_all(sp.join("guardedpkg-1.0.0.dist-info")).unwrap();
    fs::write(shared_venv.join("pyvenv.cfg"), "home = /usr/bin\n").unwrap();
    fs::write(
        sp.join("guardedpkg/__init__.py"),
        concat!(
            "import time\n",
            "class Client:\n",
            "    def fetch(self, sync: bool = False):\n",
            "        if sync:\n",
            "            time.sleep(0.1)\n",
        ),
    )
    .unwrap();
    fs::write(
        sp.join("guardedpkg-1.0.0.dist-info/METADATA"),
        "Name: guardedpkg\nVersion: 1.0.0\n",
    )
    .unwrap();
    fs::write(
        sp.join("guardedpkg-1.0.0.dist-info/RECORD"),
        "guardedpkg/__init__.py,,\n",
    )
    .unwrap();

    // Seed PackageCache with a helper function inside guardedpkg that calls Client().fetch(...)
    fs::write(
        sp.join("guardedpkg/api.py"),
        concat!(
            "from . import Client\n",
            "def safe_api():\n",
            "    Client().fetch(False)\n",
            "def unsafe_api():\n",
            "    Client().fetch(True)\n",
        ),
    )
    .unwrap();
    fs::write(
        sp.join("guardedpkg-1.0.0.dist-info/RECORD"),
        "guardedpkg/__init__.py,,\nguardedpkg/api.py,,\n",
    )
    .unwrap();

    let proj1 = temp_project(&[(
        "app.py",
        "from guardedpkg.api import safe_api, unsafe_api\nasync def s():\n    safe_api()\nasync def u():\n    unsafe_api()\n",
    )]);
    #[cfg(unix)]
    std::os::unix::fs::symlink(&shared_venv, proj1.join(".venv")).unwrap();

    let cache = fresh_cache_dir("bound-guard-cache");
    let cold = scan_path_with_cache(&proj1, Some(&cache)).unwrap();
    let cold_entries: Vec<&str> = cold
        .candidates
        .iter()
        .map(|c| c.entry.name.as_str())
        .collect();
    assert_eq!(
        cold_entries,
        vec!["u"],
        "cold candidates (installed third-party branch sinks are candidates; False is dropped): {:?}",
        cold.candidates
    );

    // Force whole-scan miss while hitting warm PackageCache.
    fs::write(
        proj1.join("app.py"),
        "# force scan miss\nfrom guardedpkg.api import safe_api, unsafe_api\nasync def s():\n    safe_api()\nasync def u():\n    unsafe_api()\n",
    )
    .unwrap();
    let warm = scan_path_with_cache(&proj1, Some(&cache)).unwrap();
    let warm_entries: Vec<&str> = warm
        .candidates
        .iter()
        .map(|c| c.entry.name.as_str())
        .collect();
    assert_eq!(
        warm_entries,
        vec!["u"],
        "warm PackageCache must preserve is_bound_call for ParamGuard: {:?}",
        warm.candidates
    );

    let _ = fs::remove_dir_all(&proj1);
    let _ = fs::remove_dir_all(&shared_venv);
    let _ = fs::remove_dir_all(&cache);
}

#[test]
fn single_file_scan_bypasses_whole_scan_cache_when_imported_sibling_changes() {
    let dir = temp_project(&[
        ("helper.py", "def work():\n    return 1\n"),
        (
            "app.py",
            "from helper import work\n\nasync def handler():\n    work()\n",
        ),
    ]);
    let cache = fresh_cache_dir("single-file-sibling");
    let target = dir.join("app.py");

    let r1 = scan_path_with_cache(&target, Some(&cache)).unwrap();
    assert!(r1.findings.is_empty(), "initial scan should be clean");

    // Mutate imported sibling module without touching app.py.
    fs::write(
        dir.join("helper.py"),
        "import time\ndef work():\n    time.sleep(0.5)\n",
    )
    .unwrap();

    // Note: single-file extract only extracts app.py as first-party unless ty resolves helper;
    // test with a stub `helper.pyi` that changes type inference on `app.py`!
    fs::write(
        dir.join("helper.pyi"),
        "import requests\ndef get_session() -> requests.Session: ...\n",
    )
    .unwrap();
    fs::write(
        &target,
        "from helper import get_conn\n\nasync def handler():\n    get_conn().execute('SELECT 1')\n",
    )
    .unwrap();

    // First scan with stub typing get_conn() -> object (clean)
    fs::write(dir.join("helper.pyi"), "def get_conn() -> object: ...\n").unwrap();
    let clean = scan_path_with_cache(&target, Some(&cache)).unwrap();
    assert!(clean.findings.is_empty(), "object stub should be clean");

    // Now edit ONLY sibling helper.pyi to return stdlib sqlite3.Connection
    fs::write(
        dir.join("helper.pyi"),
        "import sqlite3\ndef get_conn() -> sqlite3.Connection: ...\n",
    )
    .unwrap();
    let updated = scan_path_with_cache(&target, Some(&cache)).unwrap();
    assert!(
        !updated.findings.is_empty(),
        "single-file scan must not return stale cached report when sibling helper.pyi changes"
    );

    let _ = fs::remove_dir_all(&dir);
    let _ = fs::remove_dir_all(&cache);
}

#[test]
fn site_packages_namespace_and_single_file_packages_get_distinct_metadata() {
    let dir = tempfile::tempdir().unwrap();
    let sp = dir.path().join("lib/python3.12/site-packages");
    fs::create_dir_all(sp.join("google/cloud/storage")).unwrap();
    fs::create_dir_all(sp.join("google/protobuf")).unwrap();
    fs::create_dir_all(sp.join("google_cloud_storage-2.14.0.dist-info")).unwrap();
    fs::create_dir_all(sp.join("protobuf-4.25.0.dist-info")).unwrap();
    fs::create_dir_all(sp.join("six-1.16.0.dist-info")).unwrap();

    let gcs_py = sp.join("google/cloud/storage/client.py");
    let pb_py = sp.join("google/protobuf/descriptor.py");
    let six_py = sp.join("six.py");
    fs::write(&gcs_py, "def upload(): pass\n").unwrap();
    fs::write(&pb_py, "def parse(): pass\n").unwrap();
    fs::write(&six_py, "PY3 = True\n").unwrap();

    fs::write(
        sp.join("google_cloud_storage-2.14.0.dist-info/METADATA"),
        "Name: google-cloud-storage\nVersion: 2.14.0\n",
    )
    .unwrap();
    fs::write(
        sp.join("google_cloud_storage-2.14.0.dist-info/RECORD"),
        "google/cloud/storage/client.py,sha256=aaa,19\n",
    )
    .unwrap();

    fs::write(
        sp.join("protobuf-4.25.0.dist-info/METADATA"),
        "Name: protobuf\nVersion: 4.25.0\n",
    )
    .unwrap();
    fs::write(
        sp.join("protobuf-4.25.0.dist-info/RECORD"),
        "google/protobuf/descriptor.py,sha256=bbb,18\n",
    )
    .unwrap();

    fs::write(
        sp.join("six-1.16.0.dist-info/METADATA"),
        "Name: six\nVersion: 1.16.0\n",
    )
    .unwrap();
    fs::write(
        sp.join("six-1.16.0.dist-info/RECORD"),
        "six.py,sha256=ccc,11\n",
    )
    .unwrap();

    let mut memo = PackageMetaMemo::new();
    let gcs_meta = memo.meta_for_path(&gcs_py).unwrap();
    let pb_meta = memo.meta_for_path(&pb_py).unwrap();
    let six_meta = memo.meta_for_path(&six_py).unwrap();

    assert_eq!(
        (gcs_meta.0.as_str(), gcs_meta.1.as_str()),
        ("google-cloud-storage", "2.14.0")
    );
    assert_eq!(
        (pb_meta.0.as_str(), pb_meta.1.as_str()),
        ("protobuf", "4.25.0")
    );
    assert_ne!(
        gcs_meta.2, pb_meta.2,
        "distinct namespace dists must have distinct fingerprints"
    );
    assert_eq!(
        (six_meta.0.as_str(), six_meta.1.as_str()),
        ("six", "1.16.0")
    );
}

#[test]
fn scan_fingerprint_invalidates_on_outside_extra_path_and_pth_changes() {
    let ws = fresh_cache_dir("fp-extra-ws");
    let shared = ws.join("shared_ext");
    let proj = ws.join("app_proj");
    let sp = proj.join(".venv/lib/python3.12/site-packages");
    fs::create_dir_all(&shared).unwrap();
    fs::create_dir_all(&sp).unwrap();
    fs::write(proj.join(".venv/pyvenv.cfg"), "home = /usr/bin\n").unwrap();

    let ext_py = shared.join("extmod.py");
    fs::write(&ext_py, "def helper() -> int:\n    return 1\n").unwrap();
    fs::write(
        proj.join("pyrightconfig.json"),
        r#"{"extraPaths": ["../shared_ext"]}"#,
    )
    .unwrap();
    let app_py = proj.join("app.py");
    fs::write(
        &app_py,
        "import extmod\nasync def h():\n    extmod.helper()\n",
    )
    .unwrap();

    let files = vec![app_py.clone()];
    let fp0 = crate::hermetic_env::with_clean_venv_env(|| {
        scan_input_fingerprint(&proj, &files, AnalyzeOptions::default(), 256)
    });

    // Edit outside extraPath module — must invalidate fingerprint!
    fs::write(
        &ext_py,
        "import time\ndef helper() -> int:\n    time.sleep(1)\n    return 1\n",
    )
    .unwrap();
    let fp1 = crate::hermetic_env::with_clean_venv_env(|| {
        scan_input_fingerprint(&proj, &files, AnalyzeOptions::default(), 256)
    });
    assert_ne!(
        fp0, fp1,
        "editing .py inside outside extraPaths must invalidate scan_input_fingerprint"
    );

    // Add/edit a .pth file in site-packages — must invalidate fingerprint!
    fs::write(sp.join("custom.pth"), "/some/custom/path\n").unwrap();
    let fp2 = crate::hermetic_env::with_clean_venv_env(|| {
        scan_input_fingerprint(&proj, &files, AnalyzeOptions::default(), 256)
    });
    assert_ne!(
        fp1, fp2,
        "adding/editing a .pth file in site-packages must invalidate scan_input_fingerprint"
    );

    let _ = fs::remove_dir_all(&ws);
}

#[test]
fn warm_package_cache_preserves_zero_edge_init_and_dist_packages_file_paths() {
    let venv = fresh_cache_dir("zero-edge-file-venv");
    let sp = venv.join("lib/python3.12/site-packages");
    let dp = venv.join("lib/python3.12/dist-packages");
    fs::create_dir_all(sp.join("zeropkg")).unwrap();
    fs::create_dir_all(sp.join("zeropkg-1.0.0.dist-info")).unwrap();
    fs::create_dir_all(dp.join("distpkg")).unwrap();
    fs::create_dir_all(dp.join("distpkg-1.0.0.dist-info")).unwrap();
    fs::write(venv.join("pyvenv.cfg"), "home = /usr/bin\n").unwrap();

    fs::write(
        sp.join("zeropkg/__init__.py"),
        "def noop():\n    return 1\n",
    )
    .unwrap();
    fs::write(
        sp.join("zeropkg-1.0.0.dist-info/METADATA"),
        "Name: zeropkg\nVersion: 1.0.0\n",
    )
    .unwrap();
    fs::write(
        sp.join("zeropkg-1.0.0.dist-info/RECORD"),
        "zeropkg/__init__.py,,\n",
    )
    .unwrap();

    fs::write(
        dp.join("distpkg/__init__.py"),
        "def noop():\n    return 2\n",
    )
    .unwrap();
    fs::write(
        dp.join("distpkg-1.0.0.dist-info/METADATA"),
        "Name: distpkg\nVersion: 1.0.0\n",
    )
    .unwrap();
    fs::write(
        dp.join("distpkg-1.0.0.dist-info/RECORD"),
        "distpkg/__init__.py,,\n",
    )
    .unwrap();

    let proj1 = temp_project(&[(
        "app.py",
        "import zeropkg, distpkg\nasync def h():\n    zeropkg.noop()\n    distpkg.noop()\n",
    )]);
    let proj2 = temp_project(&[(
        "app.py",
        "# warm pkg cache\nimport zeropkg, distpkg\nasync def h2():\n    zeropkg.noop()\n    distpkg.noop()\n",
    )]);
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&venv, proj1.join(".venv")).unwrap();
        std::os::unix::fs::symlink(&venv, proj2.join(".venv")).unwrap();
    }

    let cache = fresh_cache_dir("zero-edge-file-cache");
    let cold = crate::extract_path_with_cache(&proj1, Some(&cache)).unwrap();
    let warm = crate::extract_path_with_cache(&proj2, Some(&cache)).unwrap();

    for (label, facts) in [("cold", &cold), ("warm", &warm)] {
        let z = facts
            .functions
            .iter()
            .find(|f| f.module == "zeropkg" && f.qualname == "noop")
            .unwrap();
        let d = facts
            .functions
            .iter()
            .find(|f| f.module == "distpkg" && f.qualname == "noop")
            .unwrap();
        assert_eq!(
            z.file, "site-packages/zeropkg/__init__.py",
            "{label} zeropkg file"
        );
        assert_eq!(
            d.file, "dist-packages/distpkg/__init__.py",
            "{label} distpkg file"
        );
    }

    let _ = fs::remove_dir_all(&proj1);
    let _ = fs::remove_dir_all(&proj2);
    let _ = fs::remove_dir_all(&venv);
    let _ = fs::remove_dir_all(&cache);
}

#[test]
fn warm_package_cache_resolves_cross_package_callbacks_independent_of_pkg_order() {
    let venv = fresh_cache_dir("cb-order-venv");
    let sp = venv.join("lib/python3.12/site-packages");
    for d in [
        "ahop",
        "ahop-1.0.0.dist-info",
        "liba",
        "liba-1.0.0.dist-info",
        "libb",
        "libb-1.0.0.dist-info",
    ] {
        fs::create_dir_all(sp.join(d)).unwrap();
    }
    fs::write(venv.join("pyvenv.cfg"), "home = /usr/bin\n").unwrap();

    fs::write(
        sp.join("ahop/__init__.py"),
        "import liba\ndef hop():\n    liba.trigger()\n",
    )
    .unwrap();
    fs::write(
        sp.join("ahop-1.0.0.dist-info/METADATA"),
        "Name: ahop\nVersion: 1.0.0\n",
    )
    .unwrap();
    fs::write(
        sp.join("ahop-1.0.0.dist-info/RECORD"),
        "ahop/__init__.py,,\n",
    )
    .unwrap();

    fs::write(
        sp.join("liba/__init__.py"),
        concat!(
            "from libb import sync_pos, sync_kw\n",
            "def run_cbs(pos_cb, *, kw_cb=None):\n",
            "    pos_cb()\n",
            "    if kw_cb is not None:\n",
            "        kw_cb()\n",
            "def trigger():\n",
            "    run_cbs(sync_pos, kw_cb=sync_kw)\n",
        ),
    )
    .unwrap();
    fs::write(
        sp.join("liba-1.0.0.dist-info/METADATA"),
        "Name: liba\nVersion: 1.0.0\n",
    )
    .unwrap();
    fs::write(
        sp.join("liba-1.0.0.dist-info/RECORD"),
        "liba/__init__.py,,\n",
    )
    .unwrap();

    fs::write(
        sp.join("libb/__init__.py"),
        concat!(
            "import time\n",
            "def sync_pos():\n    time.sleep(0.1)\n",
            "def sync_kw():\n    open('x.txt')\n",
        ),
    )
    .unwrap();
    fs::write(
        sp.join("libb-1.0.0.dist-info/METADATA"),
        "Name: libb\nVersion: 1.0.0\n",
    )
    .unwrap();
    fs::write(
        sp.join("libb-1.0.0.dist-info/RECORD"),
        "libb/__init__.py,,\n",
    )
    .unwrap();

    // Seed: Wave 0 extracts ahop.hop + libb.sync_pos + libb.sync_kw; Wave 1 extracts liba.trigger (seeing libb in def_map!).
    let seed_proj = temp_project(&[(
        "app.py",
        concat!(
            "import ahop, libb\n",
            "async def seed_b():\n    libb.sync_pos()\n    libb.sync_kw()\n",
            "async def seed_a():\n    ahop.hop()\n",
        ),
    )]);
    // Warm: Wave 0 materializes liba BEFORE libb (`"liba" < "libb"`).
    let warm_proj = temp_project(&[(
        "app.py",
        concat!(
            "import liba, libb\n",
            "async def route_a():\n    liba.trigger()\n",
            "async def route_b():\n    libb.sync_pos()\n    libb.sync_kw()\n",
        ),
    )]);
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&venv, seed_proj.join(".venv")).unwrap();
        std::os::unix::fs::symlink(&venv, warm_proj.join(".venv")).unwrap();
    }

    let cache = fresh_cache_dir("cb-order-cache");
    let seeded = scan_path_with_cache(&seed_proj, Some(&cache)).unwrap();
    let seed_a_rules: Vec<&str> = seeded
        .findings
        .iter()
        .filter(|f| f.entry.name == "seed_a")
        .map(|f| f.rule.as_str())
        .collect();
    assert!(
        seed_a_rules.contains(&"blocking-sleep"),
        "seed_a findings={seeded:?}"
    );

    let warm = scan_path_with_cache(&warm_proj, Some(&cache)).unwrap();
    let route_a_findings: Vec<&str> = warm
        .findings
        .iter()
        .filter(|f| f.entry.name == "route_a")
        .map(|f| f.rule.as_str())
        .collect();
    assert!(
        route_a_findings.contains(&"blocking-sleep"),
        "warm positional callback must survive liba < libb order: {:?}",
        warm.findings
    );
    let route_a_cands: Vec<&str> = warm
        .candidates
        .iter()
        .filter(|c| c.entry.name == "route_a")
        .map(|c| c.rule.as_str())
        .collect();
    assert!(
        !route_a_cands.is_empty() || route_a_findings.len() >= 2,
        "warm keyword callback must survive liba < libb order: findings={:?} candidates={:?}",
        warm.findings,
        warm.candidates
    );

    let _ = fs::remove_dir_all(&seed_proj);
    let _ = fs::remove_dir_all(&warm_proj);
    let _ = fs::remove_dir_all(&venv);
    let _ = fs::remove_dir_all(&cache);
}
