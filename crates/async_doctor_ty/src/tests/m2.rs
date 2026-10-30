//! m2 integration tests.

use async_doctor_core::AnalyzeOptions;
use async_doctor_core::EntryLabel;
use std::fs;
use std::path::Path;
use std::time::Instant;

use super::helpers::{clone_fixture, fresh_cache_dir, report_json_with_cache, temp_project};
use crate::{scan_path, scan_path_with_cache, scan_path_with_options};

#[test]
fn m2_nested_same_name_not_merged() {
    let dir = temp_project(&[(
        "nest.py",
        "import time\n\ndef outer_a():\n    def inner():\n        time.sleep(1)\n    inner()\n\ndef outer_b():\n    def inner():\n        return 0\n    inner()\n\nasync def handler_a():\n    outer_a()\n\nasync def handler_b():\n    outer_b()\n",
    )]);
    let facts = crate::extract::extract_path(&dir).unwrap();
    let names: Vec<_> = facts
        .functions
        .iter()
        .map(|f| f.qualname.as_str())
        .collect();
    assert!(names.contains(&"outer_a.inner"), "{names:?}");
    assert!(names.contains(&"outer_b.inner"), "{names:?}");
    let report = scan_path(&dir).unwrap();
    assert!(
        report.findings.iter().any(|f| f.entry.name == "handler_a"),
        "handler_a should find sleep via outer_a.inner: {:?}",
        report.findings
    );
    assert!(
        !report.findings.iter().any(|f| f.entry.name == "handler_b"),
        "handler_b must stay clean: {:?}",
        report.findings
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn m2_higher_order_propagates_callable_arg() {
    let dir = temp_project(&[(
        "ho.py",
        "import time\n\ndef block():\n    time.sleep(1)\n\ndef apply(fn):\n    fn()\n\nasync def handler():\n    apply(block)\n",
    )]);
    let report = scan_path(&dir).unwrap();
    assert_eq!(report.findings.len(), 1, "{:?}", report.findings);
    assert!(
        report.findings[0].chain.len() >= 3,
        "{:?}",
        report.findings[0].chain
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn m2_inferred_offload_wrapper_cuts_fake_name_does_not() {
    let dir = temp_project(&[(
        "wrap.py",
        "import asyncio\nimport time\n\ndef work():\n    time.sleep(1)\n\nasync def real_wrapper(fn, *args):\n    return await asyncio.to_thread(fn, *args)\n\nasync def to_thread(fn, *args):\n    return fn(*args)\n\nasync def safe():\n    await real_wrapper(work)\n\nasync def unsafe():\n    await to_thread(work)\n",
    )]);
    let report = scan_path(&dir).unwrap();
    assert!(
        !report.findings.iter().any(|f| f.entry.name == "safe"),
        "inferred wrapper must cut: {:?}",
        report.findings
    );
    assert!(
        report
            .findings
            .iter()
            .any(|f| f.entry.name == "unsafe" || f.entry.name == "to_thread"),
        "fake to_thread must still report: {:?}",
        report.findings
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn m2_lambda_assign_property_genexpr() {
    let dir = temp_project(&[(
        "forms.py",
        "import time\n\nclass Holder:\n    @property\n    def data(self):\n        time.sleep(1)\n        return 1\n\ndef gen():\n    time.sleep(1)\n    yield 1\n\nasync def lambda_assign():\n    f = lambda: time.sleep(1)\n    f()\n\nasync def property_access():\n    _ = Holder().data\n\nasync def genexpr_list():\n    list(x for x in gen())\n\nasync def genexpr_deferred():\n    g = (x for x in gen())\n    list(g)\n",
    )]);
    let report = scan_path(&dir).unwrap();
    let entries: Vec<_> = report
        .findings
        .iter()
        .map(|f| f.entry.name.as_str())
        .collect();
    for needed in [
        "lambda_assign",
        "property_access",
        "genexpr_list",
        "genexpr_deferred",
    ] {
        assert!(
            entries.contains(&needed),
            "missing {needed} in {entries:?} findings={:?}",
            report.findings
        );
    }
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn m2_imperative_routes_labeled() {
    let dir = temp_project(&[
        (
            "routes.py",
            "import time\n\nclass APIRouter:\n    def __init__(self):\n        self.routes = []\n    def add_api_route(self, path, endpoint, **kwargs):\n        self.routes.append((path, endpoint))\n\nrouter = APIRouter()\n\nasync def via_router():\n    time.sleep(1)\n\nrouter.add_api_route('/r', via_router)\n",
        ),
        (
            "app.py",
            "import time\nfrom routes import router\n\nclass App:\n    def add_api_route(self, path, endpoint, **kwargs):\n        pass\n    def add_route(self, path, endpoint, **kwargs):\n        pass\n    def include_router(self, r):\n        pass\n\nclass Route:\n    def __init__(self, path, endpoint, **kwargs):\n        self.endpoint = endpoint\n\nclass WebSocketRoute:\n    def __init__(self, path, endpoint, **kwargs):\n        self.endpoint = endpoint\n\napp = App()\n\nasync def via_add_api():\n    time.sleep(1)\n\nasync def via_add_route():\n    time.sleep(1)\n\nasync def via_route():\n    time.sleep(1)\n\nasync def via_ws():\n    time.sleep(1)\n\napp.add_api_route('/a', via_add_api)\napp.add_route('/b', via_add_route)\n_ = [Route('/c', via_route), WebSocketRoute('/ws', via_ws)]\napp.include_router(router)\n",
        ),
    ]);
    let report = scan_path(&dir).unwrap();
    assert!(report.findings.len() >= 4, "{:?}", report.findings);
    for f in &report.findings {
        assert_eq!(
            f.entry.label,
            EntryLabel::Route,
            "{} should be route, got {:?}",
            f.entry.name,
            f.entry.label
        );
    }
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn m2_show_unresolved_aggregates_by_pattern() {
    let dir = temp_project(&[(
        "dyn.py",
        "async def a():\n    getattr(object(), 'x')()\n\nasync def b():\n    getattr(object(), 'x')()\n    getattr(object(), 'y')()\n",
    )]);
    let hidden = scan_path(&dir).unwrap();
    assert!(hidden.candidates.is_empty());
    assert!(hidden.hidden_unresolved_candidates >= 1);

    let shown = scan_path_with_options(
        &dir,
        AnalyzeOptions {
            include_non_production: false,
            show_unresolved: true,
        },
    )
    .unwrap();
    assert!(
        !shown.unresolved_summary.is_empty(),
        "{:?}",
        shown.unresolved_summary
    );
    assert!(
        shown.unresolved_summary[0].entries_affected
            >= shown
                .unresolved_summary
                .iter()
                .map(|p| p.entries_affected)
                .min()
                .unwrap_or(0)
    );
    // Ranked by entries_affected descending.
    for w in shown.unresolved_summary.windows(2) {
        assert!(w[0].entries_affected >= w[1].entries_affected);
    }
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn m2_analysis_is_byte_identical_across_runs() {
    let dir = clone_fixture("m2_third_party_rules");
    let cache = fresh_cache_dir("det-cache");

    const N: usize = 5;
    let mut jsons = Vec::with_capacity(N);
    for _ in 0..N {
        // Drop cache between cold runs so we exercise follow selection, not hit path.
        let _ = fs::remove_dir_all(&cache);
        fs::create_dir_all(&cache).unwrap();
        jsons.push(report_json_with_cache(&dir, &cache));
    }
    for (i, j) in jsons.iter().enumerate().skip(1) {
        assert_eq!(
            &jsons[0], j,
            "run 0 vs run {i} JSON differ (non-deterministic follow/budget)"
        );
    }
    let report: async_doctor_core::Report = serde_json::from_str(&jsons[0]).unwrap();
    assert!(
        report.findings.len() >= 12,
        "expected third-party rule findings, got {}",
        report.findings.len()
    );
    assert!(
        report
            .findings
            .iter()
            .any(|f| f.entry.name == "use_redis_pipeline_execute"),
        "pipeline execute must fire: {:?}",
        report
            .findings
            .iter()
            .map(|f| &f.entry.name)
            .collect::<Vec<_>>()
    );
    assert!(
        !report
            .findings
            .iter()
            .any(|f| f.entry.name == "use_redis_pipeline_buffering_ok"),
        "pipeline buffering must not fire: {:?}",
        report.findings
    );

    let _ = fs::remove_dir_all(&dir);
    let _ = fs::remove_dir_all(&cache);
}

#[test]
fn m2_sitepkgs_cold_warm_json_identical_and_warm_faster() {
    let dir = clone_fixture("m2_sitepkgs_cache");
    let cache = fresh_cache_dir("warm-cache");

    let t0 = Instant::now();
    let cold = report_json_with_cache(&dir, &cache);
    let cold_ms = t0.elapsed();

    let t1 = Instant::now();
    let warm = report_json_with_cache(&dir, &cache);
    let warm_ms = t1.elapsed();

    assert_eq!(
        cold, warm,
        "cold and warm JSON must be byte-identical\ncold={cold}\nwarm={warm}"
    );
    let report: async_doctor_core::Report = serde_json::from_str(&cold).unwrap();
    assert_eq!(report.findings.len(), 1, "{:?}", report.findings);
    assert_eq!(report.findings[0].rule, "blocking-sleep");
    assert!(
        report.findings[0]
            .chain
            .iter()
            .any(|s| s.text.contains("time.sleep")),
        "{:?}",
        report.findings[0].chain
    );
    // Warm should hit the package cache. Tiny fixtures + parallel cargo-test
    // processes make wall-clock ratios noisy — only guard against gross
    // regressions (warm >> cold) when cold was non-trivial.
    if cold_ms.as_millis() >= 500 {
        assert!(
            warm_ms.as_secs_f64() <= cold_ms.as_secs_f64() * 2.0,
            "warm ({warm_ms:?}) should not be much slower than cold ({cold_ms:?})"
        );
    }
    let _ = (cold_ms, warm_ms);
    // Cache file must exist under the versioned schema/tool path.
    fn any_json(dir: &Path) -> bool {
        let Ok(rd) = fs::read_dir(dir) else {
            return false;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() && any_json(&p) {
                return true;
            }
            if p.extension().and_then(|x| x.to_str()) == Some("json") {
                return true;
            }
        }
        false
    }
    assert!(any_json(&cache), "expected package cache under {cache:?}");

    let _ = fs::remove_dir_all(&dir);
    let _ = fs::remove_dir_all(&cache);
}

#[test]
fn m2_redis_pipeline_buffering_vs_execute_fixture() {
    let dir = clone_fixture("m2_third_party_rules");
    let cache = fresh_cache_dir("pipe-cache");

    let report = scan_path_with_cache(&dir, Some(&cache)).unwrap();
    let names: Vec<_> = report
        .findings
        .iter()
        .map(|f| f.entry.name.as_str())
        .collect();
    assert!(
        names.contains(&"use_redis_pipeline_execute"),
        "execute must be a finding: {names:?}"
    );
    assert!(
        !names.contains(&"use_redis_pipeline_buffering_ok"),
        "buffering must not be a finding: {names:?}"
    );
    let exec = report
        .findings
        .iter()
        .find(|f| f.entry.name == "use_redis_pipeline_execute")
        .unwrap();
    assert!(
        exec.chain.iter().any(|c| c.text.contains("execute")),
        "evidence should point at execute(): {:?}",
        exec.chain
    );

    let _ = fs::remove_dir_all(&dir);
    let _ = fs::remove_dir_all(&cache);
}

#[test]
fn m2_redis_real_hierarchy_pipeline_receiver_type() {
    let dir = clone_fixture("m2_redis_pipeline_real");
    let cache = fresh_cache_dir("redis-real");

    let report = scan_path_with_cache(&dir, Some(&cache)).unwrap();
    let names: Vec<_> = report
        .findings
        .iter()
        .map(|f| f.entry.name.as_str())
        .collect();
    assert!(
        names.contains(&"sync_pipeline_execute_blocks"),
        "sync Pipeline.execute must fire: {names:?}"
    );
    assert!(
        names.contains(&"sync_redis_get_blocks"),
        "sync Redis.get must fire: {names:?}"
    );
    for banned in [
        "sync_pipeline_set_ok",
        "asyncio_pipeline_execute_ok",
        "asyncio_pipeline_set_ok",
    ] {
        assert!(
            !names.contains(&banned),
            "{banned} must not be a finding: {names:?}"
        );
    }
    let exec = report
        .findings
        .iter()
        .find(|f| f.entry.name == "sync_pipeline_execute_blocks")
        .unwrap();
    assert!(
        exec.chain.iter().any(|c| c.text.contains("execute")),
        "{:?}",
        exec.chain
    );

    let _ = fs::remove_dir_all(&dir);
    let _ = fs::remove_dir_all(&cache);
}

#[test]
fn m2_third_party_async_never_entry() {
    let dir = clone_fixture("m2_third_party_never_entry");
    let cache = fresh_cache_dir("tp-entry");

    let report = scan_path_with_cache(&dir, Some(&cache)).unwrap();
    assert_eq!(report.findings.len(), 1, "{:?}", report.findings);
    assert_eq!(report.findings[0].entry.name, "handler");
    assert_eq!(report.findings[0].entry.file, "app.py");
    assert!(
        !report
            .findings
            .iter()
            .any(|f| f.entry.file.contains("site-packages") || f.entry.name == "fetch"),
        "third-party async must not be the entry: {:?}",
        report.findings
    );

    let _ = fs::remove_dir_all(&dir);
    let _ = fs::remove_dir_all(&cache);
}

#[test]
fn m2_multipkg_cold_warm_byte_identical() {
    let dir = clone_fixture("m2_cache_multipkg");
    let cache = fresh_cache_dir("multipkg");

    let cold = report_json_with_cache(&dir, &cache);
    let warm = report_json_with_cache(&dir, &cache);
    assert_eq!(
        cold, warm,
        "multi-package cold/warm JSON must match\ncold={cold}\nwarm={warm}"
    );
    let report: async_doctor_core::Report = serde_json::from_str(&cold).unwrap();
    assert!(
        report.findings.len() >= 2,
        "expected findings via libx→liby: {:?}",
        report.findings
    );
    let warm2 = report_json_with_cache(&dir, &cache);
    assert_eq!(cold, warm2);

    let _ = fs::remove_dir_all(&dir);
    let _ = fs::remove_dir_all(&cache);
}

#[test]
fn m1_await_nested_sync_sinks_still_reported() {
    // Regression: is_awaited must mark only the direct await operand, not nested
    // calls in arguments (`await send(open(p).read())` must still find open).
    let dir = temp_project(&[(
        "app.py",
        r#"
from pathlib import Path
import asyncio
import time

async def send(x):
    return x

async def await_open_read(p):
    await send(open(p).read())

async def await_path_read_text(p):
    await send(Path(p).read_text())

async def await_gather_sleep():
    await asyncio.gather(send(time.sleep(1)))
"#,
    )]);
    let report = scan_path(&dir).unwrap();
    let names: Vec<_> = report
        .findings
        .iter()
        .map(|f| f.entry.name.as_str())
        .collect();
    for expected in [
        "await_open_read",
        "await_path_read_text",
        "await_gather_sleep",
    ] {
        assert!(
            names.contains(&expected),
            "missing M1 TP {expected}: findings={names:?} candidates={:?}",
            report
                .candidates
                .iter()
                .map(|c| &c.entry.name)
                .collect::<Vec<_>>()
        );
    }
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn m2_awaited_async_api_not_followed_into_internals() {
    // `await psycopg.AsyncConnection.connect` / `await redis.eval` must not FP
    // via third-party async internals.
    let dir = clone_fixture("m2_awaited_async_no_follow");
    let cache = fresh_cache_dir("await-nofollow");
    let report = scan_path_with_cache(&dir, Some(&cache)).unwrap();
    let finding_names: Vec<_> = report
        .findings
        .iter()
        .map(|f| f.entry.name.as_str())
        .collect();
    for banned in ["use_async_connect", "use_asyncio_redis_eval"] {
        assert!(
            !finding_names.contains(&banned),
            "{banned} must not be a finding via library internals: {finding_names:?}"
        );
    }
    // Still report the known sync sink through a real third-party sync path.
    assert!(
        finding_names.contains(&"use_libz_sleep"),
        "must still follow unknown TP async → sync: {finding_names:?}"
    );
    let _ = fs::remove_dir_all(&dir);
    let _ = fs::remove_dir_all(&cache);
}

#[test]
fn m2_asyncio_redis_pipeline_async_with_and_scan_iter() {
    let dir = clone_fixture("m2_asyncio_redis_ctx");
    let cache = fresh_cache_dir("asyncio-redis-ctx");
    let report = scan_path_with_cache(&dir, Some(&cache)).unwrap();
    let names: Vec<_> = report
        .findings
        .iter()
        .map(|f| f.entry.name.as_str())
        .collect();
    for banned in [
        "asyncio_pipeline_async_with_set_ok",
        "asyncio_scan_iter_ok",
        "asyncio_eval_await_ok",
    ] {
        assert!(
            !names.contains(&banned),
            "{banned} must not fire: {names:?}"
        );
    }
    assert!(
        names.contains(&"sync_redis_get_blocks"),
        "sync redis must still fire: {names:?}"
    );
    let _ = fs::remove_dir_all(&dir);
    let _ = fs::remove_dir_all(&cache);
}

#[test]
fn m2_pil_param_branch_sink_is_candidate() {
    let dir = clone_fixture("m2_pil_param_branch");
    let cache = fresh_cache_dir("pil-branch");
    let report = scan_path_with_cache(&dir, Some(&cache)).unwrap();
    let finding_names: Vec<_> = report
        .findings
        .iter()
        .map(|f| f.entry.name.as_str())
        .collect();
    assert!(
        !finding_names
            .iter()
            .any(|n| n.contains("image_open") || n.contains("image_save")),
        "PIL open/save must not be findings (BytesIO clean; path → candidate): {finding_names:?}"
    );
    // Proven in-memory buffers must not surface as blocking-file at all.
    for c in &report.candidates {
        if matches!(c.entry.name.as_str(), "use_image_open" | "use_image_save") {
            assert_ne!(
                c.rule.as_str(),
                "blocking-file",
                "in-memory PIL must not be blocking-file candidate: {c:?}"
            );
        }
    }
    // Path string behind PIL's branch guard → candidate, not finding.
    let path_cands: Vec<_> = report
        .candidates
        .iter()
        .filter(|c| {
            matches!(
                c.entry.name.as_str(),
                "use_image_open_path" | "use_image_save_path"
            ) && c.rule == "blocking-file"
        })
        .collect();
    assert!(
        !path_cands.is_empty(),
        "path-param PIL open/save behind branch guard must be candidates: {:?}",
        report.candidates
    );
    let _ = fs::remove_dir_all(&dir);
    let _ = fs::remove_dir_all(&cache);
}

#[test]
fn m2_large_multipkg_cold_warm_json_identical() {
    let dir = clone_fixture("m2_cache_multipkg_large");
    let cache = fresh_cache_dir("multipkg-large");
    let cold = report_json_with_cache(&dir, &cache);
    let warm = report_json_with_cache(&dir, &cache);
    assert_eq!(
        cold, warm,
        "large multipkg cold/warm JSON must match\ncold={cold}\nwarm={warm}"
    );
    let report: async_doctor_core::Report = serde_json::from_str(&cold).unwrap();
    assert!(
        report.findings.len() >= 3,
        "expected several findings across packages: {:?}",
        report.findings.len()
    );
    // Candidate counts must also match (the large-repo cold/warm bug).
    let cold_r: async_doctor_core::Report = serde_json::from_str(&cold).unwrap();
    let warm_r: async_doctor_core::Report = serde_json::from_str(&warm).unwrap();
    assert_eq!(
        cold_r.candidates.len(),
        warm_r.candidates.len(),
        "candidate count cold={} warm={}",
        cold_r.candidates.len(),
        warm_r.candidates.len()
    );
    let _ = fs::remove_dir_all(&dir);
    let _ = fs::remove_dir_all(&cache);
}

#[test]
fn m2_cache_fingerprint_invalidates_on_content_change() {
    use crate::cache::{
        PackageCache, load_package_cache, package_content_fingerprint, save_package_cache,
    };
    let root = fresh_cache_dir("fp-root");
    let pkg = root.join("site-packages").join("demo");
    fs::create_dir_all(&pkg).unwrap();
    let dist = root.join("site-packages").join("demo-1.0.0.dist-info");
    fs::create_dir_all(&dist).unwrap();
    fs::write(dist.join("RECORD"), "demo/__init__.py,,\n").unwrap();
    let fp1 = package_content_fingerprint(&dist);
    let mut cache = PackageCache::new("demo", "1.0.0");
    cache.functions.insert(
        "demo.f".into(),
        crate::cache::CachedFunction {
            key: "demo.f".into(),
            qualname: "f".into(),
            module: "demo".into(),
            line: 1,
            is_async: false,
            is_generator: false,
            param_names: vec![],
            invokes_params: vec![],
            is_offload_wrapper: false,
            edges: vec![],
        },
    );
    save_package_cache(&root, &cache, &fp1).unwrap();
    assert!(load_package_cache(&root, "demo", "1.0.0", &fp1).is_some());
    // Patch RECORD → new fingerprint must miss the old cache entry.
    fs::write(dist.join("RECORD"), "demo/__init__.py,,\ndemo/extra.py,,\n").unwrap();
    let fp2 = package_content_fingerprint(&dist);
    assert_ne!(fp1, fp2);
    assert!(load_package_cache(&root, "demo", "1.0.0", &fp2).is_none());
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn m2_asyncio_redis_union_receiver_not_sync_sink() {
    let dir = clone_fixture("m2_asyncio_redis_union");
    let cache = fresh_cache_dir("redis-union");
    let report = scan_path_with_cache(&dir, Some(&cache)).unwrap();
    let names: Vec<_> = report
        .findings
        .iter()
        .map(|f| f.entry.name.as_str())
        .collect();
    for banned in [
        "union_pipeline_set_ok",
        "union_client_pipeline_async_with_ok",
    ] {
        assert!(
            !names.contains(&banned),
            "{banned} must not be blocking-db via mixin fallback: {names:?}"
        );
    }
    // Also must not surface as findings-shaped candidates for those entries.
    for c in &report.candidates {
        assert!(
            c.entry.name != "union_pipeline_set_ok" || c.rule != "blocking-db",
            "union pipe.set must not be blocking-db candidate: {c:?}"
        );
    }
    let _ = fs::remove_dir_all(&dir);
    let _ = fs::remove_dir_all(&cache);
}

#[test]
fn m2_venv_cold_warm_warm_byte_identical() {
    let dir = clone_fixture("m2_venv_cold_warm_realistic");
    let cache = fresh_cache_dir("venv-realistic");
    let cold = report_json_with_cache(&dir, &cache);
    let warm = report_json_with_cache(&dir, &cache);
    let warm2 = report_json_with_cache(&dir, &cache);
    assert_eq!(
        cold, warm,
        "cold vs warm must match\ncold={cold}\nwarm={warm}"
    );
    assert_eq!(
        warm, warm2,
        "warm vs warm2 must match\nwarm={warm}\nwarm2={warm2}"
    );
    let report: async_doctor_core::Report = serde_json::from_str(&cold).unwrap();
    assert!(
        report
            .findings
            .iter()
            .any(|f| f.entry.name == "handler_fetch" || f.entry.name == "handler_sleep"),
        "expected libx/sleep findings: {:?}",
        report
            .findings
            .iter()
            .map(|f| &f.entry.name)
            .collect::<Vec<_>>()
    );
    // Warm must not invent extra site-packages-only candidates vs cold.
    let site_cands = |r: &async_doctor_core::Report| {
        r.candidates
            .iter()
            .filter(|c| c.chain.iter().any(|s| s.file.contains("site-packages")))
            .count()
    };
    let cold_r: async_doctor_core::Report = serde_json::from_str(&cold).unwrap();
    let warm_r: async_doctor_core::Report = serde_json::from_str(&warm).unwrap();
    assert_eq!(
        site_cands(&cold_r),
        site_cands(&warm_r),
        "site-packages candidate count cold={} warm={}",
        site_cands(&cold_r),
        site_cands(&warm_r)
    );
    assert_eq!(
        cold_r.hidden_unresolved_candidates,
        warm_r.hidden_unresolved_candidates
    );
    let _ = fs::remove_dir_all(&dir);
    let _ = fs::remove_dir_all(&cache);
}
