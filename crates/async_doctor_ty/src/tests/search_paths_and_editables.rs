//! Nested editable packages, search-path safety, and attribute receiver resolution.

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
fn m3c_sitepkgs_cold_warm_hidden_unresolved_identical() {
    // Locks cold==warm including hiddenUnresolvedCandidates (third-party follow).
    let dir = clone_fixture("m2_sitepkgs_cache");
    let cache = fresh_cache_dir("m3c-sitepkgs-cw");
    let cold = report_json_with_cache(&dir, &cache);
    let warm = report_json_with_cache(&dir, &cache);
    assert_eq!(
        cold, warm,
        "cold/warm must match including hidden unresolved counts"
    );
    let report: async_doctor_core::Report = serde_json::from_str(&cold).unwrap();
    assert_eq!(report.findings.len(), 1, "{:?}", report.findings);
    let _ = fs::remove_dir_all(&dir);
    let _ = fs::remove_dir_all(&cache);
}

#[test]
fn m3c_missing_third_party_is_one_aggregate_note() {
    // Missing third-party packages never emit per-import records; they live in
    // locationLessCandidates (always visible) rather than hidden candidates.
    let dir = temp_project(&[(
        "main.py",
        "from missing_pkg.cfg import load_config\nimport also_missing\n\nasync def handler():\n    return load_config()\n",
    )]);
    let hidden = scan_path(&dir).unwrap();
    assert!(
        hidden.findings.is_empty(),
        "unresolved must not be a finding: {:?}",
        hidden.findings
    );
    assert_eq!(
        hidden.location_less_candidates.len(),
        1,
        "exactly one location-less third-party note, got {:?}",
        hidden.location_less_candidates
    );
    assert_eq!(
        hidden.environment.unresolved_third_party, 2,
        "environment should count both missing modules: {:?}",
        hidden.environment
    );
    assert!(
        hidden.candidates.is_empty(),
        "plain unresolved calls must stay hidden without --show-unresolved: {:?}",
        hidden.candidates
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
            .unresolved_summary
            .iter()
            .any(|p| { p.pattern.contains("third-party modules not installed") && p.sites == 2 }),
        "expected one aggregate note covering 2 modules: {:?}",
        shown.unresolved_summary
    );
    assert_eq!(
        shown
            .location_less_candidates
            .iter()
            .filter(|c| c.rule == "unresolved-import")
            .count(),
        1,
        "one aggregate location-less candidate, not per-import: {:?}",
        shown.location_less_candidates
    );
    assert!(
        shown
            .candidates
            .iter()
            .all(|c| c.rule != "unresolved-call" || c.entry.name != "handler"),
        "must not emit per-call unresolved for handler: {:?}",
        shown.candidates
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn m3c_first_party_unresolved_import_surfaces() {
    // Head module exists under scan root but sub-module fails → real silent-drop case.
    let dir = temp_project(&[
        ("mypkg/__init__.py", ""),
        (
            "main.py",
            "from mypkg.missing_sub import load\n\nasync def handler():\n    return load()\n",
        ),
    ]);
    let hidden = scan_path(&dir).unwrap();
    assert_eq!(
        hidden.hidden_unresolved_candidates, 1,
        "expected one first-party unresolved-import, got {}",
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
            .unresolved_summary
            .iter()
            .any(|p| { p.pattern == "unresolved-import:mypkg.missing_sub" && p.sites == 1 }),
        "{:?}",
        shown.unresolved_summary
    );
    assert!(
        shown
            .candidates
            .iter()
            .any(|c| c.rule == "unresolved-import" && c.entry.name.contains("mypkg.missing_sub")),
        "{:?}",
        shown.candidates
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn m3c_unresolved_import_dedupes_per_file_module() {
    // Same missing first-party module at top-level + inside a function → count 1.
    let dir = temp_project(&[
        ("mypkg/__init__.py", ""),
        (
            "main.py",
            "from mypkg.missing import foo\n\nasync def handler():\n    from mypkg.missing import foo as bar\n    return foo() or bar()\n",
        ),
    ]);
    let hidden = scan_path(&dir).unwrap();
    assert_eq!(
        hidden.hidden_unresolved_candidates, 1,
        "dedupe per file+module: got {}",
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
    let import_cands: Vec<_> = shown
        .candidates
        .iter()
        .filter(|c| c.rule == "unresolved-import")
        .collect();
    assert_eq!(
        import_cands.len(),
        1,
        "display must match count: {:?}",
        shown.candidates
    );
    assert!(
        shown
            .unresolved_summary
            .iter()
            .any(|p| p.pattern == "unresolved-import:mypkg.missing" && p.sites == 1),
        "{:?}",
        shown.unresolved_summary
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn m3c_first_party_unresolved_cold_warm_identical() {
    // Cold==warm including hiddenUnresolvedCandidates when unresolved-import
    // records are present (ordering / materialization must be deterministic).
    let dir = temp_project(&[
        ("mypkg/__init__.py", ""),
        (
            "main.py",
            "from mypkg.a_missing import a\nfrom mypkg.z_missing import z\n\nasync def handler():\n    return a() or z()\n",
        ),
        (
            "other.py",
            "from mypkg.a_missing import a\n\nasync def other():\n    return a()\n",
        ),
    ]);
    let cache = fresh_cache_dir("m3c-fp-unresolved-cw");
    let cold = report_json_with_cache(&dir, &cache);
    let warm = report_json_with_cache(&dir, &cache);
    assert_eq!(
        cold, warm,
        "cold/warm must match including hidden unresolved-import counts"
    );
    let report: async_doctor_core::Report = serde_json::from_str(&cold).unwrap();
    assert_eq!(
        report.hidden_unresolved_candidates, 3,
        "a_missing×2 files + z_missing×1 = 3, got {}",
        report.hidden_unresolved_candidates
    );
    let _ = fs::remove_dir_all(&dir);
    let _ = fs::remove_dir_all(&cache);
}

#[test]
fn m3c_outside_editable_never_called_not_entry() {
    // Outside editable roots are follow-only: never-called async workers there
    // must not become findings.
    let dir = clone_fixture("m3c_editable_outside_no_entry");
    let app = dir.join("app");
    let report = scan_path(&app).unwrap();
    assert!(
        report.findings.is_empty(),
        "unrelated_worker outside scan root must not be an entry: {:?}",
        report.findings
    );
    assert!(
        !report
            .findings
            .iter()
            .any(|f| f.entry.name.contains("unrelated_worker")),
        "{:?}",
        report.findings
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn m3c_package_root_self_import_resolves() {
    // Scanning a package dir (has __init__.py): parent is on the search path
    // so `import <pkg>.…` self-imports resolve.
    let outer = temp_project(&[
        ("mypkg/__init__.py", ""),
        (
            "mypkg/jobs.py",
            "import time\n\ndef work():\n    time.sleep(1)\n",
        ),
        (
            "mypkg/app.py",
            "from mypkg.jobs import work\n\nasync def handler():\n    work()\n",
        ),
    ]);
    let pkg = outer.join("mypkg");
    let report = scan_path(&pkg).unwrap();
    assert_eq!(
        report.findings.len(),
        1,
        "self-import mypkg.jobs must resolve when scanning package root: {:?}",
        report.findings
    );
    assert_eq!(report.findings[0].rule, "blocking-sleep");
    assert_eq!(report.findings[0].entry.name, "handler");
    let _ = fs::remove_dir_all(&outer);
}

#[test]
fn m3c_ruff_src_dot_keeps_findings() {
    let dir = clone_fixture("m3c_ruff_src_dot");
    let report = scan_path(&dir).unwrap();
    assert_eq!(
        report.findings.len(),
        2,
        "src=['.'] must not drop bot.store sinks: {:?}",
        report.findings
    );
    assert!(
        report
            .findings
            .iter()
            .all(|f| f.rule == "blocking-db" && f.entry.name == "handle"),
        "{:?}",
        report.findings
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

#[test]
fn m3c_editable_outside_scan_root_resolves() {
    // Package lives outside the scan root; finder MAPPING points at it.
    let pkg = temp_project(&[
        ("libcore/__init__.py", ""),
        (
            "libcore/cfg.py",
            "import time\nimport requests\n\ndef _fetch(key):\n    for attempt in range(3):\n        try:\n            return requests.get(f'https://config.example.com/{key}', timeout=5).text\n        except Exception:\n            time.sleep(0.5 * 2 ** attempt)\n    return ''\n\ndef load_config():\n    return {'secret': _fetch('secrets')}\n",
        ),
    ]);
    let mapped = pkg.join("libcore");
    let scan = temp_project(&[
        (
            "main.py",
            "from libcore.cfg import load_config\n\nasync def handler():\n    return load_config()\n\nasync def other():\n    return load_config()\n",
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
            &format!(
                "MAPPING = {{'libcore': '{}'}}\\nNAMESPACES = {{}}\\n",
                mapped.display()
            ),
        ),
        (
            ".venv/lib/python3.12/site-packages/__editable__.libcore-0.1.0.pth",
            "import __editable___libcore_0_1_0_finder\n",
        ),
    ]);
    // Fix accidental escaped newline from format — rewrite finder cleanly.
    fs::write(
        scan.join(".venv/lib/python3.12/site-packages/__editable___libcore_0_1_0_finder.py"),
        format!(
            "MAPPING = {{'libcore': r'{}'}}\nNAMESPACES = {{}}\n",
            mapped.display()
        ),
    )
    .unwrap();

    let report = scan_path(&scan).unwrap();
    assert_four_http_sleep(&report, "outside-scan-root editable");
    let _ = fs::remove_dir_all(&scan);
    let _ = fs::remove_dir_all(&pkg);
}

#[test]
fn m3c_relimport_pkg_root_zero_unresolved() {
    // Scan root is the package: existing relative imports must not be FPs.
    let dir = clone_fixture("m3c_relimport_pkg_root");
    let hidden = scan_path(&dir).unwrap();
    assert_eq!(
        hidden.hidden_unresolved_candidates, 0,
        "existing relative imports must not be unresolved: hidden={}",
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
    let import_cands: Vec<_> = shown
        .candidates
        .iter()
        .filter(|c| c.rule == "unresolved-import")
        .collect();
    assert!(
        import_cands.is_empty(),
        "expected 0 unresolved-import, got {}: {:?}",
        import_cands.len(),
        import_cands
    );
    assert!(
        shown
            .unresolved_summary
            .iter()
            .all(|p| !p.pattern.starts_with("unresolved-import:")),
        "{:?}",
        shown.unresolved_summary
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn m3c_relimport_missing_is_true_positive() {
    let dir = clone_fixture("m3c_relimport_missing");
    let hidden = scan_path(&dir).unwrap();
    assert_eq!(
        hidden.hidden_unresolved_candidates, 1,
        "missing relative import must still report: {}",
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
            .unresolved_summary
            .iter()
            .any(|p| { p.pattern == "unresolved-import:.missing_mod" && p.sites == 1 }),
        "{:?}",
        shown.unresolved_summary
    );
    assert!(
        shown
            .candidates
            .iter()
            .any(|c| c.rule == "unresolved-import" && c.entry.name.contains("missing_mod")),
        "{:?}",
        shown.candidates
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn monorepo_subdirectory_scan_discovers_ancestor_dot_venv_and_venv() {
    for venv_dir_name in [".venv", "venv"] {
        let repo = temp_project(&[(
            "packages/api/routes.py",
            concat!(
                "import asyncio\n",
                "import httpx\n",
                "import monopkg\n\n",
                "async def handler():\n",
                "    monopkg.block_now()\n",
                "    await asyncio.to_thread(httpx.get, 'https://example.com')\n",
            ),
        )]);
        // temp_project has no .venv by default, create at repo root only.
        let sp = repo
            .join(venv_dir_name)
            .join("lib")
            .join("python3.12")
            .join("site-packages");
        fs::create_dir_all(sp.join("monopkg")).unwrap();
        fs::create_dir_all(sp.join("httpx")).unwrap();
        fs::write(
            repo.join(venv_dir_name).join("pyvenv.cfg"),
            "home = /usr/bin\n",
        )
        .unwrap();
        fs::write(
            sp.join("monopkg/__init__.py"),
            "import time\ndef block_now():\n    time.sleep(0.1)\n",
        )
        .unwrap();
        fs::write(
            sp.join("httpx/__init__.py"),
            concat!(
                "class Client:\n    def get(self, *a, **k): pass\n",
                "class AsyncClient:\n    async def get(self, *a, **k): pass\n",
                "def get(url, *a, **k):\n    return Client().get(url)\n",
            ),
        )
        .unwrap();

        let sub = repo.join("packages/api");
        let report = scan_path(&sub).unwrap();
        assert_eq!(
            report.environment.kind,
            async_doctor_core::EnvironmentKind::Venv,
            "ancestor {venv_dir_name} must be detected as Venv: {:?}",
            report.environment
        );
        assert!(
            report
                .findings
                .iter()
                .any(|f| f.rule == "blocking-sleep" && f.entry.name == "handler"),
            "ancestor {venv_dir_name} site-packages must be followed: {:?}",
            report.findings
        );
        assert!(
            report
                .advice
                .iter()
                .any(|a| a.rule == "offload-has-async-twin" && a.entry.name == "handler"),
            "ancestor {venv_dir_name} must also feed twin_resolve: {:?}",
            report.advice
        );
        let _ = fs::remove_dir_all(&repo);
    }
}

#[test]
fn pdm_pypackages_and_external_poetry_pipenv_venvs_resolve() {
    // 1. PDM PEP 582 `__pypackages__/<ver>/lib` on an ancestor directory.
    let pdm_repo = temp_project(&[(
        "services/worker/job.py",
        "import pdmpkg\nasync def run():\n    pdmpkg.do_io()\n",
    )]);
    let pdm_lib = pdm_repo.join("__pypackages__/3.12/lib");
    fs::create_dir_all(pdm_lib.join("pdmpkg")).unwrap();
    fs::write(
        pdm_lib.join("pdmpkg/__init__.py"),
        "import time\ndef do_io():\n    time.sleep(0.1)\n",
    )
    .unwrap();
    let pdm_report = scan_path(&pdm_repo.join("services/worker")).unwrap();
    assert!(
        pdm_report
            .findings
            .iter()
            .any(|f| f.rule == "blocking-sleep" && f.entry.name == "run"),
        "PDM __pypackages__ must resolve third-party package: {:?}",
        pdm_report.findings
    );
    let _ = fs::remove_dir_all(&pdm_repo);

    // 2. External Poetry (`POETRY_VIRTUALENVS_PATH`) & Pipenv (`WORKON_HOME`) stores.
    let store = fresh_cache_dir("ext-venv-stores");
    let poetry_store = store.join("pypoetry-venvs");
    let pipenv_store = store.join("pipenv-venvs");
    let poetry_proj = temp_project(&[
        ("pyproject.toml", "[tool.poetry]\nname = \"MyApp\"\n"),
        (
            "sub/app.py",
            "import poetrypkg\nasync def p():\n    poetrypkg.sync_work()\n",
        ),
    ]);
    let p_canon = poetry_proj.canonicalize().unwrap_or(poetry_proj.clone());
    let p_hash = crate::env_discovery::sha256_base64url8(p_canon.to_string_lossy().as_bytes());
    let p_sp = poetry_store.join(format!(
        "myapp-{p_hash}-py3.12/lib/python3.12/site-packages"
    ));
    fs::create_dir_all(p_sp.join("poetrypkg")).unwrap();
    fs::write(
        p_sp.join("poetrypkg/__init__.py"),
        "import time\ndef sync_work():\n    time.sleep(0.1)\n",
    )
    .unwrap();

    let pipenv_proj = temp_project(&[
        ("Pipfile", "[packages]\npipenvpkg = \"*\"\n"),
        (
            "app.py",
            "import pipenvpkg\nasync def q():\n    pipenvpkg.sync_work()\n",
        ),
    ]);
    let pip_canon = pipenv_proj.canonicalize().unwrap_or(pipenv_proj.clone());
    let pip_hash = crate::env_discovery::sha256_base64url8(
        pip_canon.join("Pipfile").to_string_lossy().as_bytes(),
    );
    let dir_name = pipenv_proj.file_name().unwrap().to_str().unwrap();
    let e_sp = pipenv_store.join(format!(
        "{dir_name}-{pip_hash}/lib/python3.12/site-packages"
    ));
    fs::create_dir_all(e_sp.join("pipenvpkg")).unwrap();
    fs::write(
        e_sp.join("pipenvpkg/__init__.py"),
        "import time\ndef sync_work():\n    time.sleep(0.1)\n",
    )
    .unwrap();

    crate::hermetic_env::with_clean_venv_env(|| {
        unsafe {
            std::env::set_var("POETRY_VIRTUALENVS_PATH", &poetry_store);
            std::env::set_var("WORKON_HOME", &pipenv_store);
        }
        let r1 = scan_path(&poetry_proj.join("sub")).unwrap();
        let r2 = scan_path(&pipenv_proj).unwrap();
        unsafe {
            std::env::remove_var("POETRY_VIRTUALENVS_PATH");
            std::env::remove_var("WORKON_HOME");
        }
        assert_eq!(r1.environment.origin, "poetry virtualenv");
        assert!(r1.findings.iter().any(|f| f.rule == "blocking-sleep"));
        assert_eq!(r2.environment.origin, "pipenv virtualenv");
        assert!(r2.findings.iter().any(|f| f.rule == "blocking-sleep"));

        // 3. Default `$HOME/.local/share/virtualenvs` (Pipenv) and
        // `$HOME/Library/Caches/pypoetry/virtualenvs` (macOS Poetry) still resolve
        // even when `XDG_CACHE_HOME` is set and `XDG_DATA_HOME` is unset.
        let fake_home = store.join("fake-home");
        let xdg_cache = store.join("custom-xdg-cache");
        let home_pipenv = fake_home.join(".local/share/virtualenvs").join(format!(
            "{dir_name}-{pip_hash}/lib/python3.12/site-packages"
        ));
        let home_poetry = fake_home
            .join("Library/Caches/pypoetry/virtualenvs")
            .join(format!(
                "myapp-{p_hash}-py3.12/lib/python3.12/site-packages"
            ));
        fs::create_dir_all(home_pipenv.join("pipenvpkg")).unwrap();
        fs::create_dir_all(home_poetry.join("poetrypkg")).unwrap();
        fs::create_dir_all(&xdg_cache).unwrap();
        fs::write(
            home_pipenv.join("pipenvpkg/__init__.py"),
            "import time\ndef sync_work():\n    time.sleep(0.1)\n",
        )
        .unwrap();
        fs::write(
            home_poetry.join("poetrypkg/__init__.py"),
            "import time\ndef sync_work():\n    time.sleep(0.1)\n",
        )
        .unwrap();
        let old_home = std::env::var_os("HOME");
        let old_xdg_cache = std::env::var_os("XDG_CACHE_HOME");
        let old_xdg_data = std::env::var_os("XDG_DATA_HOME");
        unsafe {
            std::env::set_var("HOME", &fake_home);
            std::env::set_var("XDG_CACHE_HOME", &xdg_cache);
            std::env::remove_var("XDG_DATA_HOME");
        }
        let res = std::panic::catch_unwind(|| {
            let rp = scan_path(&poetry_proj.join("sub")).unwrap();
            let re = scan_path(&pipenv_proj).unwrap();
            assert_eq!(rp.environment.origin, "poetry virtualenv");
            assert!(rp.findings.iter().any(|f| f.rule == "blocking-sleep"));
            assert_eq!(re.environment.origin, "pipenv virtualenv");
            assert!(re.findings.iter().any(|f| f.rule == "blocking-sleep"));
        });
        unsafe {
            match old_home {
                Some(v) => std::env::set_var("HOME", v),
                None => std::env::remove_var("HOME"),
            }
            match old_xdg_cache {
                Some(v) => std::env::set_var("XDG_CACHE_HOME", v),
                None => std::env::remove_var("XDG_CACHE_HOME"),
            }
            match old_xdg_data {
                Some(v) => std::env::set_var("XDG_DATA_HOME", v),
                None => std::env::remove_var("XDG_DATA_HOME"),
            }
        }
        if let Err(payload) = res {
            std::panic::resume_unwind(payload);
        }
    });
    let _ = fs::remove_dir_all(&poetry_proj);
    let _ = fs::remove_dir_all(&pipenv_proj);
    let _ = fs::remove_dir_all(&store);
}
