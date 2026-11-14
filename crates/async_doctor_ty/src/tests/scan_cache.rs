//! Whole-scan report cache: warm hit, invalidation, stale schema.

use std::fs;
use std::time::Instant;

use super::helpers::{clone_fixture, fresh_cache_dir, report_json_with_cache, temp_project};

#[test]
fn scan_cache_warm_is_much_faster_when_unchanged() {
    let dir = clone_fixture("m2_sitepkgs_cache");
    let cache = fresh_cache_dir("scan-warm-fast");

    let (cold, cold_ms, warm, warm_ms) = crate::hermetic_env::with_clean_venv_env(|| {
        let t0 = Instant::now();
        let cold = report_json_with_cache(&dir, &cache);
        let cold_ms = t0.elapsed();

        let t1 = Instant::now();
        let warm = report_json_with_cache(&dir, &cache);
        let warm_ms = t1.elapsed();
        (cold, cold_ms, warm, warm_ms)
    });

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

#[test]
fn ephemeral_tool_runner_path_pythons_skipped_for_system_dist_packages() {
    use crate::scan_path;
    use async_doctor_core::AnalyzeOptions;

    let ws = fresh_cache_dir("ephemeral-path-ws");
    let uvx_bin = ws.join("uv/archive-v0/ephem123/bin");
    let uvx_sp = ws.join("uv/archive-v0/ephem123/lib/python3.12/site-packages");
    let pipx_bin = ws.join("pipx/venvs/async-doctor/bin");
    let pipx_sp = ws.join("pipx/venvs/async-doctor/lib/python3.12/site-packages");
    let self_only_bin = ws.join("custom-runner/bin");
    let self_only_sp = ws.join("custom-runner/lib/python3.12/site-packages");
    let docker_bin = ws.join("docker-usr/bin");
    let docker_dp = ws.join("docker-usr/lib/python3/dist-packages");
    for dir in [
        &uvx_bin,
        &uvx_sp,
        &pipx_bin,
        &pipx_sp,
        &self_only_bin,
        &docker_bin,
    ] {
        fs::create_dir_all(dir).unwrap();
    }
    for dir in [
        self_only_sp.join("async_doctor-0.2.0.dist-info"),
        self_only_sp.join("pip-24.0.dist-info"),
        docker_dp.join("dockerpkg"),
        docker_dp.join("dockerpkg-1.0.0.dist-info"),
        docker_dp.join("httpx"),
        docker_dp.join("httpx-0.27.0.dist-info"),
    ] {
        fs::create_dir_all(dir).unwrap();
    }
    for bin_dir in [&uvx_bin, &pipx_bin, &self_only_bin, &docker_bin] {
        let exe = bin_dir.join("python3");
        fs::write(&exe, "#!/bin/sh\nexit 0\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&exe, fs::Permissions::from_mode(0o755)).unwrap();
        }
    }
    fs::write(
        docker_dp.join("dockerpkg/__init__.py"),
        "import time\ndef sync_call():\n    time.sleep(0.1)\n",
    )
    .unwrap();
    fs::write(
        docker_dp.join("dockerpkg-1.0.0.dist-info/METADATA"),
        "Name: dockerpkg\nVersion: 1.0.0\n",
    )
    .unwrap();
    fs::write(
        docker_dp.join("dockerpkg-1.0.0.dist-info/RECORD"),
        "dockerpkg/__init__.py,,\n",
    )
    .unwrap();
    fs::write(
        docker_dp.join("httpx/__init__.py"),
        concat!(
            "class Client:\n    def get(self, *a, **k): pass\n",
            "class AsyncClient:\n    async def get(self, *a, **k): pass\n",
            "def get(url, *a, **k):\n    return Client().get(url)\n",
        ),
    )
    .unwrap();
    fs::write(
        docker_dp.join("httpx-0.27.0.dist-info/METADATA"),
        "Name: httpx\nVersion: 0.27.0\n",
    )
    .unwrap();
    fs::write(
        docker_dp.join("httpx-0.27.0.dist-info/RECORD"),
        "httpx/__init__.py,,\n",
    )
    .unwrap();

    let proj = temp_project(&[(
        "app.py",
        concat!(
            "import asyncio\nimport dockerpkg\nimport httpx\n\n",
            "async def endpoint():\n",
            "    dockerpkg.sync_call()\n",
            "    await asyncio.to_thread(httpx.get, 'https://example.com')\n",
        ),
    )]);
    crate::hermetic_env::with_clean_venv_env(|| {
        let old_path = std::env::var_os("PATH");
        let new_path =
            std::env::join_paths([&uvx_bin, &pipx_bin, &self_only_bin, &docker_bin]).unwrap();
        unsafe { std::env::set_var("PATH", &new_path) };
        let res = std::panic::catch_unwind(|| {
            let report = scan_path(&proj).unwrap();
            assert!(
                report.findings.iter().any(|f| f.rule == "blocking-sleep"),
                "{report:?}"
            );
            assert!(
                report
                    .advice
                    .iter()
                    .any(|a| a.rule == "offload-has-async-twin"),
                "{:?}",
                report.advice
            );
            let files = vec![proj.join("app.py")];
            let fp1 = crate::scan_cache::scan_input_fingerprint(
                &proj,
                &files,
                AnalyzeOptions::default(),
                256,
            );
            fs::write(
                docker_dp.join("dockerpkg-1.0.0.dist-info/RECORD"),
                "dockerpkg/__init__.py,sha256=changed,42\n",
            )
            .unwrap();
            let fp2 = crate::scan_cache::scan_input_fingerprint(
                &proj,
                &files,
                AnalyzeOptions::default(),
                256,
            );
            assert_ne!(
                fp1, fp2,
                "scan_cache must fingerprint system/Docker dist-packages"
            );
        });
        unsafe {
            match old_path {
                Some(p) => std::env::set_var("PATH", p),
                None => std::env::remove_var("PATH"),
            }
        }
        if let Err(payload) = res {
            std::panic::resume_unwind(payload);
        }
    });
    let _ = fs::remove_dir_all(&proj);
    let _ = fs::remove_dir_all(&ws);
}

#[test]
fn scan_cache_gc_retains_at_most_three_per_root_and_prunes_stale_schemas() {
    let cache = fresh_cache_dir("scan-gc-k3");
    fs::create_dir_all(cache.join("v1/0.1.0/scan")).unwrap();
    fs::create_dir_all(cache.join("v21/0.1.2/scan")).unwrap();
    fs::write(cache.join("v21/0.1.2/scan/old.json"), "{}").unwrap();

    let proj_a = temp_project(&[("app.py", "async def a():\n    return 0\n")]);
    let proj_b = temp_project(&[("app.py", "async def b():\n    return 99\n")]);

    let _ = report_json_with_cache(&proj_b, &cache);
    for step in 1..=4 {
        fs::write(
            proj_a.join("app.py"),
            format!("async def a():\n    return {step}\n"),
        )
        .unwrap();
        let _ = report_json_with_cache(&proj_a, &cache);
    }

    assert!(
        !cache.join("v1").exists() && !cache.join("v21").exists(),
        "save_scan_report must opportunistically prune sibling schema dirs < v{}",
        async_doctor_ty_cache_schema()
    );

    let scan_dir = cache
        .join(format!("v{}", async_doctor_ty_cache_schema()))
        .join(env!("CARGO_PKG_VERSION"))
        .join("scan");
    let json_files: Vec<String> = fs::read_dir(&scan_dir)
        .unwrap()
        .flatten()
        .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("json"))
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        json_files.len(),
        4,
        "expected K=3 entries for proj_a + 1 untouched entry for proj_b, got {json_files:?}"
    );

    let _ = fs::remove_dir_all(&proj_a);
    let _ = fs::remove_dir_all(&proj_b);
    let _ = fs::remove_dir_all(&cache);
}

#[test]
fn unpackaged_subdir_and_subsec_mtime_invalidate_package_content_fingerprint() {
    use crate::cache::package_content_fingerprint;
    use std::time::{Duration, UNIX_EPOCH};

    let dir = fresh_cache_dir("unpackaged-subdir-fp");
    let pkg = dir.join("mypkg");
    fs::create_dir_all(pkg.join("sub")).unwrap();
    fs::write(pkg.join("__init__.py"), "from .sub import worker\n").unwrap();
    let sub_py = pkg.join("sub/worker.py");
    fs::write(&sub_py, "def run():\n    return 1\n").unwrap();

    let fp0 = package_content_fingerprint(&pkg);
    fs::write(&sub_py, "import time\ndef run():\n    time.sleep(1)\n").unwrap();
    let fp1 = package_content_fingerprint(&pkg);
    assert_ne!(
        fp0, fp1,
        "editing subdir .py inside unpackaged package with __init__.py must change fingerprint"
    );

    let single = dir.join("loose_mod.py");
    fs::write(&single, "x = 1\n").unwrap();
    let f = fs::OpenOptions::new().write(true).open(&single).unwrap();
    f.set_modified(UNIX_EPOCH + Duration::new(1_700_000_000, 100_000_000))
        .unwrap();
    let s0 = package_content_fingerprint(&single);
    f.set_modified(UNIX_EPOCH + Duration::new(1_700_000_000, 900_000_000))
        .unwrap();
    let s1 = package_content_fingerprint(&single);
    assert_ne!(
        s0, s1,
        "sub-second nanosecond mtime change must change package_content_fingerprint"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn follow_third_party_groups_same_file_functions_into_single_parse() {
    let proj = temp_project(&[(
        "app.py",
        concat!(
            "from multifn import client\n",
            "async def handler():\n",
            "    client.f1()\n",
            "    client.f2()\n",
            "    client.f3()\n",
            "    client.f4()\n",
            "    client.f5()\n",
        ),
    )]);
    let sp = proj.join(".venv/lib/python3.12/site-packages");
    fs::create_dir_all(sp.join("multifn")).unwrap();
    fs::create_dir_all(sp.join("multifn-1.0.0.dist-info")).unwrap();
    fs::write(proj.join(".venv/pyvenv.cfg"), "home = /usr/bin\n").unwrap();
    fs::write(sp.join("multifn/__init__.py"), "").unwrap();
    fs::write(
        sp.join("multifn/client.py"),
        concat!(
            "import time\n",
            "def f5():\n    time.sleep(0.05)\n",
            "def f1():\n    return f5()\n",
            "def f2():\n    return 2\n",
            "def f3():\n    open('a.txt')\n",
            "def f4():\n    return 4\n",
        ),
    )
    .unwrap();
    fs::write(
        sp.join("multifn-1.0.0.dist-info/METADATA"),
        "Name: multifn\nVersion: 1.0.0\n",
    )
    .unwrap();
    fs::write(
        sp.join("multifn-1.0.0.dist-info/RECORD"),
        "multifn/__init__.py,,\nmultifn/client.py,,\n",
    )
    .unwrap();

    let cache = fresh_cache_dir("multifn-grouped-parse");
    crate::extract::follow::reset_third_party_file_parse_count();
    let cold_json = report_json_with_cache(&proj, &cache);
    let parses = crate::extract::follow::third_party_file_parse_count();
    assert_eq!(
        parses, 1,
        "all 5 functions in multifn/client.py must be extracted in 1 file parse, got {parses}"
    );

    // Force whole-scan miss while hitting warm PackageCache and verify exact parity.
    fs::write(
        proj.join("app.py"),
        concat!(
            "# warm package cache check\n",
            "from multifn import client\n",
            "async def handler():\n",
            "    client.f1()\n",
            "    client.f2()\n",
            "    client.f3()\n",
            "    client.f4()\n",
            "    client.f5()\n",
        ),
    )
    .unwrap();
    crate::extract::follow::reset_third_party_file_parse_count();
    let warm_json = report_json_with_cache(&proj, &cache);
    assert_eq!(
        crate::extract::follow::third_party_file_parse_count(),
        0,
        "warm PackageCache hit must not re-parse multifn/client.py"
    );
    let cold_v: serde_json::Value = serde_json::from_str(&cold_json).unwrap();
    let warm_v: serde_json::Value = serde_json::from_str(&warm_json).unwrap();
    assert_eq!(
        cold_v["findings"].as_array().unwrap().len(),
        warm_v["findings"].as_array().unwrap().len()
    );
    assert!(cold_v["findings"].as_array().unwrap().len() >= 2);

    let _ = fs::remove_dir_all(&proj);
    let _ = fs::remove_dir_all(&cache);
}

#[test]
fn wanted_scoped_cross_probes_and_disjoint_flush_union() {
    use crate::cache::{
        CachedEdge, CachedFollowTarget, CachedFunction, PackageCache, load_package_cache,
        save_package_cache,
    };
    use crate::sitepkgs::{PackageCacheBatch, materialize_cached_functions};
    use async_doctor_core::{EntryLabel, FunctionFact, FunctionId, ProjectFacts};
    use std::collections::{BTreeMap, HashMap};

    let root = fresh_cache_dir("wanted-cross-and-union");
    let fp = "fpwanted123";
    let mut cache = PackageCache::new("pkg1", "1.0.0");
    cache.functions.insert(
        "pkg1.clean".into(),
        serde_json::from_value(serde_json::json!({
            "key": "pkg1.clean",
            "qualname": "clean",
            "module": "pkg1",
            "line": 1,
            "is_async": false,
            "is_generator": false,
            "param_names": [],
            "invokes_params": [],
            "is_offload_wrapper": false,
            "decorator_quals": [],
            "edges": []
        }))
        .unwrap(),
    );
    let mut broken_cf: CachedFunction = serde_json::from_value(serde_json::json!({
        "key": "pkg1.broken",
        "qualname": "broken",
        "module": "pkg1",
        "line": 10,
        "is_async": false,
        "is_generator": false,
        "param_names": [],
        "invokes_params": [],
        "is_offload_wrapper": false,
        "decorator_quals": [],
        "edges": []
    }))
    .unwrap();
    broken_cf.edges.push(CachedEdge {
        callee_qualname: "missing_pkg.fn_x".into(),
        callee_kind: "external".into(),
        offload_kind: None,
        param_invoke_index: None,
        is_offload_wrapper: false,
        kind: "Call".into(),
        file: "site-packages/pkg1/__init__.py".into(),
        line: 11,
        text: "fn_x()".into(),
        conditions: vec!["unconditional".into()],
        occurrence: 0,
        positional_literals: vec![],
        keyword_literals: vec![],
        positional_callable_quals: vec![],
        positional_externals: vec![],
        positional_constructed_receivers: vec![],
        keyword_callable_quals: vec![],
        param_guards: vec![],
        is_awaited: false,
        callee_is_coroutine: None,
        is_eager_offload_arg: false,
        is_bound_call: false,
        constructed_receiver: None,
    });
    cache.functions.insert("pkg1.broken".into(), broken_cf);
    cache.follow_targets.insert(
        "missing_pkg.fn_x".into(),
        CachedFollowTarget {
            qualname: "missing_pkg.fn_x".into(),
            path: "/tmp/site-packages/missing_pkg/__init__.py".into(),
            name_start: 0,
            package_name: Some("missing_pkg".into()),
            package_version: Some("1.0.0".into()),
            package_fingerprint: Some("fpmissing".into()),
        },
    );
    save_package_cache(&root, &cache, fp).unwrap();

    let mut facts = ProjectFacts::default();
    let mut qual_to_id = BTreeMap::new();
    let mut id_to_ext = HashMap::new();
    let mut ft = HashMap::new();
    let mut batch = PackageCacheBatch::new(root.clone());
    let loaded = materialize_cached_functions(
        &mut facts,
        &mut batch,
        "pkg1",
        "1.0.0",
        fp,
        &["pkg1.clean".to_string()],
        &mut qual_to_id,
        &mut id_to_ext,
        &mut ft,
    );
    assert_eq!(
        loaded,
        vec!["pkg1.clean".to_string()],
        "unrelated missing cross-package target must not invalidate wanted=pkg1.clean"
    );

    let union_fp = "fpunion456";
    let mut b1 = PackageCacheBatch::new(root.clone());
    let mut b2 = PackageCacheBatch::new(root.clone());
    let make_fn = |id: u32, name: &str| FunctionFact {
        id: FunctionId(id),
        file: "site-packages/upkg/__init__.py".into(),
        line: id + 1,
        end_line: id + 1,
        module: "upkg".into(),
        qualname: name.into(),
        is_async: false,
        is_generator: false,
        label: EntryLabel::Other,
        is_sync_route: false,
        async_twins: vec![],
        param_names: vec![],
        is_third_party: true,
        package_name: Some("upkg".into()),
        package_version: Some("1.0.0".into()),
        invokes_params: vec![],
        is_offload_wrapper: false,
        offload_decorator: None,
    };
    b1.upsert_function(
        "upkg",
        "1.0.0",
        union_fp,
        "upkg.f1",
        &make_fn(0, "f1"),
        &[],
        &HashMap::new(),
        &HashMap::new(),
        vec![],
    );
    b2.upsert_function(
        "upkg",
        "1.0.0",
        union_fp,
        "upkg.f2",
        &make_fn(1, "f2"),
        &[],
        &HashMap::new(),
        &HashMap::new(),
        vec![],
    );
    b1.flush();
    b2.flush();
    let merged = load_package_cache(&root, "upkg", "1.0.0", union_fp).unwrap();
    assert!(
        merged.functions.contains_key("upkg.f1") && merged.functions.contains_key("upkg.f2"),
        "concurrent disjoint flushes must union functions, got {:?}",
        merged.functions.keys().collect::<Vec<_>>()
    );

    let _ = fs::remove_dir_all(&root);
}
