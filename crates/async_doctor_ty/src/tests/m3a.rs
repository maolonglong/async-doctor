//! M3a recall / precision gap regression tests.

use std::fs;

use super::helpers::{clone_fixture, fixtures_dir, fresh_cache_dir, temp_project};
use crate::scan_path;
use crate::scan_path_with_cache;

#[test]
fn m3a_redis_await_gather_watch_attr() {
    let dir = clone_fixture("m3a_corpus_gaps");
    let cache = fresh_cache_dir("m3a-redis");
    let report = scan_path_with_cache(&dir, Some(&cache)).unwrap();
    let names: Vec<_> = report
        .findings
        .iter()
        .map(|f| f.entry.name.as_str())
        .collect();
    for expected in [
        "redis_sync_await",
        "redis_sync_gather",
        "redis_watch",
        "redis_attr_client",
    ] {
        assert!(
            names.contains(&expected),
            "missing {expected}: findings={names:?}"
        );
    }
    assert!(
        !names.contains(&"redis_asyncio_await_ok"),
        "asyncio redis await must not fire: {names:?}"
    );
    let _ = fs::remove_dir_all(&dir);
    let _ = fs::remove_dir_all(&cache);
}

#[test]
fn m3a_begin_entered() {
    let dir = clone_fixture("m3a_corpus_gaps");
    let cache = fresh_cache_dir("m3a-begin");
    let report = scan_path_with_cache(&dir, Some(&cache)).unwrap();
    let names: Vec<_> = report
        .findings
        .iter()
        .map(|f| f.entry.name.as_str())
        .collect();
    assert!(
        names.contains(&"begin_entered"),
        "with Engine.begin() must fire: {names:?}"
    );
    assert!(
        !names.contains(&"begin_not_entered_ok"),
        "bare begin() must not fire: {names:?}"
    );
    let _ = fs::remove_dir_all(&dir);
    let _ = fs::remove_dir_all(&cache);
}

#[test]
fn m3a_listen_consumed_and_comprehension() {
    let dir = clone_fixture("m3a_corpus_gaps");
    let cache = fresh_cache_dir("m3a-listen");
    let report = scan_path_with_cache(&dir, Some(&cache)).unwrap();
    let names: Vec<_> = report
        .findings
        .iter()
        .map(|f| f.entry.name.as_str())
        .collect();
    for expected in ["listen_consumed", "listen_comprehension"] {
        assert!(
            names.contains(&expected),
            "missing {expected}: findings={names:?}"
        );
    }
    assert!(
        !names.contains(&"listen_bare_ok"),
        "bare listen() must not fire: {names:?}"
    );
    let _ = fs::remove_dir_all(&dir);
    let _ = fs::remove_dir_all(&cache);
}

#[test]
fn m3a_unsafe_async_mutation_out_of_scope() {
    let dir = clone_fixture("m3a_corpus_gaps");
    let cache = fresh_cache_dir("m3a-mutation");
    let report = scan_path_with_cache(&dir, Some(&cache)).unwrap();
    let names: Vec<_> = report
        .findings
        .iter()
        .map(|f| f.entry.name.as_str())
        .collect();
    assert!(
        !names.contains(&"unsafe_async_mutation"),
        "shared-state mutation without blocking IO is out of scope: {names:?}"
    );
    let _ = fs::remove_dir_all(&dir);
    let _ = fs::remove_dir_all(&cache);
}

#[test]
fn m3a_eager_offload_reports_and_mentions_pass_callable() {
    // Copy into an isolated project dir (scanning the fixtures/ tree as a
    // single-file project root can pull sibling .venv noise under parallel tests).
    let src = fs::read_to_string(fixtures_dir().join("m3a_eager_offload.py")).unwrap();
    let dir = temp_project(&[("m3a_eager_offload.py", &src)]);
    let report = scan_path(&dir).unwrap();
    let by_entry: Vec<_> = report
        .findings
        .iter()
        .map(|f| (f.entry.name.as_str(), f.fix.hint.as_str()))
        .collect();
    for expected in ["eager_to_thread", "eager_run_in_executor"] {
        let hit = by_entry.iter().find(|(n, _)| *n == expected);
        assert!(hit.is_some(), "missing {expected}: {by_entry:?}");
        let hint = hit.unwrap().1;
        assert!(
            hint.contains("pass the callable") || hint.contains("rather than calling"),
            "{expected} hint must mention pass-vs-call: {hint}"
        );
    }
    let names: Vec<_> = by_entry.iter().map(|(n, _)| *n).collect();
    for banned in ["passed_to_thread_ok", "passed_run_in_executor_ok"] {
        assert!(
            !names.contains(&banned),
            "{banned} must stay clean: {names:?}"
        );
    }
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn m3a_awaited_async_http_clients_never_fire() {
    let dir = clone_fixture("m3a_async_clients_ok");
    let cache = fresh_cache_dir("m3a-async-http");
    let report = scan_path_with_cache(&dir, Some(&cache)).unwrap();
    let names: Vec<_> = report
        .findings
        .iter()
        .map(|f| f.entry.name.as_str())
        .collect();
    assert!(
        names.is_empty(),
        "awaited AsyncClient / AsyncElasticsearch must stay clean: {names:?} findings={:?}",
        report.findings
    );
    assert!(
        report.candidates.iter().all(|c| c.rule != "blocking-http"),
        "no blocking-http candidates either: {:?}",
        report.candidates
    );
    let _ = fs::remove_dir_all(&dir);
    let _ = fs::remove_dir_all(&cache);
}

#[test]
fn m3a_structural_async_subclass_wrapper_openai_stream() {
    let dir = clone_fixture("m3a_async_structural_ok");
    let cache = fresh_cache_dir("m3a-struct-async");
    let report = scan_path_with_cache(&dir, Some(&cache)).unwrap();
    let names: Vec<_> = report
        .findings
        .iter()
        .map(|f| f.entry.name.as_str())
        .collect();
    for banned in [
        "neg_subclass",
        "neg_non_async_named_wrapper",
        "neg_async_openai_stream",
    ] {
        assert!(
            !names.contains(&banned),
            "{banned} must stay clean (structural async / narrow openai): {names:?} findings={:?}",
            report.findings
        );
    }
    let _ = fs::remove_dir_all(&dir);
    let _ = fs::remove_dir_all(&cache);
}

#[test]
fn m3a_asyncio_pipeline_execute_still_clean() {
    // Regression: awaited asyncio pipe.execute must not become a sync sink
    // after removing the blanket is_awaited skip.
    let dir = clone_fixture("m2_redis_pipeline_real");
    let cache = fresh_cache_dir("m3a-pipe-neg");
    let report = scan_path_with_cache(&dir, Some(&cache)).unwrap();
    let names: Vec<_> = report
        .findings
        .iter()
        .map(|f| f.entry.name.as_str())
        .collect();
    for banned in [
        "sync_pipeline_set_ok",
        "asyncio_pipeline_execute_ok",
        "asyncio_pipeline_set_ok",
        "asyncio_redis_subclass_get_ok",
        "asyncio_redis_from_import_subclass_ok",
        "asyncio_redis_alias_subclass_get_set_ok",
        "asyncio_redis_subclass_super_get_ok",
    ] {
        assert!(
            !names.contains(&banned),
            "{banned} must not fire: {names:?}"
        );
    }
    assert!(
        names.contains(&"sync_pipeline_execute_blocks"),
        "sync pipe.execute must still fire: {names:?}"
    );
    let _ = fs::remove_dir_all(&dir);
    let _ = fs::remove_dir_all(&cache);
}

#[test]
fn m3a_sync_redis_subclass_inherits_sink() {
    // Subclasses of sync redis.Redis must fire; asyncio subclasses must not.
    let dir = clone_fixture("m2_redis_pipeline_real");
    let cache = fresh_cache_dir("m3a-redis-sub");
    let report = scan_path_with_cache(&dir, Some(&cache)).unwrap();
    let names: Vec<_> = report
        .findings
        .iter()
        .map(|f| f.entry.name.as_str())
        .collect();
    for expected in [
        "sync_redis_subclass_get_blocks",
        "sync_redis_subclass_override_get_blocks",
        "sync_redis_get_blocks",
    ] {
        assert!(
            names.contains(&expected),
            "missing {expected}: findings={names:?}"
        );
    }
    assert!(
        !names.contains(&"asyncio_redis_subclass_get_ok"),
        "asyncio Redis subclass must stay clean: {names:?}"
    );
    let _ = fs::remove_dir_all(&dir);
    let _ = fs::remove_dir_all(&cache);
}

#[test]
fn m3a_asyncio_redis_subclass_variants_never_fire() {
    // Receiver-MRO family: asyncio subclasses must stay 0 even when ty resolves
    // get/set to the shared plain-def commands mixin.
    let dir = clone_fixture("m2_redis_pipeline_real");
    let cache = fresh_cache_dir("m3a-asyncio-sub");
    let report = scan_path_with_cache(&dir, Some(&cache)).unwrap();
    let names: Vec<_> = report
        .findings
        .iter()
        .map(|f| f.entry.name.as_str())
        .collect();
    for banned in [
        "asyncio_redis_subclass_get_ok",
        "asyncio_redis_from_import_subclass_ok",
        "asyncio_redis_alias_subclass_get_set_ok",
        "asyncio_redis_subclass_super_get_ok",
    ] {
        assert!(
            !names.contains(&banned),
            "{banned} must stay clean (asyncio subclass MRO): {names:?} findings={:?}",
            report.findings
        );
    }
    // Sync counterparts on the same fixture must still fire.
    for expected in [
        "sync_redis_subclass_get_blocks",
        "sync_redis_subclass_override_get_blocks",
    ] {
        assert!(
            names.contains(&expected),
            "missing sync {expected}: findings={names:?}"
        );
    }
    let _ = fs::remove_dir_all(&dir);
    let _ = fs::remove_dir_all(&cache);
}

#[test]
fn m3a_eager_inline_fixture() {
    let dir = temp_project(&[(
        "eager.py",
        "import asyncio\nimport time\n\ndef work():\n    time.sleep(1)\n    return 1\n\nasync def handler():\n    await asyncio.to_thread(work())\n",
    )]);
    let report = scan_path(&dir).unwrap();
    assert!(!report.findings.is_empty(), "eager must report");
    assert!(
        report
            .findings
            .iter()
            .any(|f| f.fix.hint.contains("pass the callable")
                || f.fix.hint.contains("rather than calling")),
        "hint={:?}",
        report.findings[0].fix.hint
    );
    let _ = fs::remove_dir_all(dir);
}
