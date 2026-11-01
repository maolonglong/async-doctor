//! Pre-release regressions: precision sinks/guards and audit fixtures.

use std::fs;

use super::helpers::{clone_fixture, fresh_cache_dir, report_json, temp_project};
use crate::{scan_path, scan_path_with_cache};

#[test]
fn sqla_redis_nonio_only_real_commit() {
    let dir = clone_fixture("sqla-redis-nonio");
    let cache = fresh_cache_dir("sqla-redis-nonio");
    let report = scan_path_with_cache(&dir, Some(&cache)).unwrap();
    let names: Vec<_> = report
        .findings
        .iter()
        .map(|f| f.entry.name.as_str())
        .collect();
    assert_eq!(
        names,
        ["real_io"],
        "only Session.commit should remain: {names:?} findings={:?}",
        report.findings
    );
    let f = &report.findings[0];
    assert_eq!(f.rule, "blocking-db");
    assert!(
        f.chain.iter().any(|c| {
            c.resolution.contains("sqlalchemy.orm.Session.commit") || c.text.contains("commit")
        }),
        "expected Session.commit label: {:?}",
        f.chain
    );
    let _ = fs::remove_dir_all(&dir);
    let _ = fs::remove_dir_all(&cache);
}

#[test]
fn nonblocking_variants_only_bounded_put() {
    let dir = clone_fixture("nonblocking-variants");
    let cache = fresh_cache_dir("nonblocking-variants");
    let report = scan_path_with_cache(&dir, Some(&cache)).unwrap();
    let names: Vec<_> = report
        .findings
        .iter()
        .map(|f| f.entry.name.as_str())
        .collect();
    assert_eq!(
        names,
        ["bounded_put"],
        "non-blocking acquire/join/unbounded put must stay clean: {names:?}"
    );
    assert!(
        report.candidates.iter().all(|c| c.rule != "blocking-lock"
            || c.entry.name == "bounded_put"
            || c.entry.name == "with_lock"),
        "unexpected lock candidates: {:?}",
        report.candidates
    );
    let _ = fs::remove_dir_all(&dir);
    let _ = fs::remove_dir_all(&cache);
}

#[test]
fn guard_fp_early_return_drops_nested_run() {
    let dir = clone_fixture("guard-fp");
    let report = scan_path(&dir).unwrap();
    assert!(
        report
            .findings
            .iter()
            .all(|f| f.rule != "nested-event-loop"),
        "early-return loop guard must drop nested-event-loop: {:?}",
        report.findings
    );
    assert!(
        report
            .candidates
            .iter()
            .all(|c| c.rule != "nested-event-loop"),
        "must not demote to candidate: {:?}",
        report.candidates
    );
    assert!(
        report
            .findings
            .iter()
            .any(|f| f.rule == "blocking-lock" && f.entry.name == "handler"),
        "t.join() must remain: {:?}",
        report.findings
    );
    assert_eq!(report.findings.len(), 1, "{:?}", report.findings);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn urllib_request_urlopen_not_urllib3_label() {
    let dir = temp_project(&[(
        "http.py",
        "import urllib.request\n\nasync def handler():\n    urllib.request.urlopen('http://example.com')\n",
    )]);
    let report = scan_path(&dir).unwrap();
    assert_eq!(report.findings.len(), 1, "{:?}", report.findings);
    let last = report.findings[0].chain.last().unwrap();
    assert!(
        last.resolution.contains("urllib.request.urlopen"),
        "stdlib urlopen must not be labelled urllib3: {:?}",
        last.resolution
    );
    assert!(
        !last.resolution.contains("urllib3"),
        "{:?}",
        last.resolution
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn redis_from_url_factory_clean_and_get_labelled() {
    let dir = clone_fixture("sqla-redis-nonio");
    // Extra file: real command label regression alongside fixture.
    fs::write(
        dir.join("cmds.py"),
        concat!(
            "import redis\n",
            "\n",
            "async def use_get():\n",
            "    redis.Redis().get('k')\n",
            "\n",
            "async def use_hget():\n",
            "    redis.Redis().hget('h', 'f')\n",
        ),
    )
    .unwrap();
    let cache = fresh_cache_dir("redis-labels");
    let report = scan_path_with_cache(&dir, Some(&cache)).unwrap();
    assert!(
        report
            .findings
            .iter()
            .all(|f| f.entry.name != "make_client"),
        "from_url factory must stay clean: {:?}",
        report.findings
    );
    let get = report
        .findings
        .iter()
        .find(|f| f.entry.name == "use_get")
        .expect("Redis.get must fire");
    assert!(
        get.chain
            .iter()
            .any(|c| c.resolution.contains("redis.Redis.get")),
        "get must be labelled redis.Redis.get: {:?}",
        get.chain
    );
    let hget = report
        .findings
        .iter()
        .find(|f| f.entry.name == "use_hget")
        .expect("Redis.hget must fire");
    assert!(
        hget.chain
            .iter()
            .any(|c| c.resolution.contains("redis.Redis.hget")),
        "hget must not be labelled execute_command: {:?}",
        hget.chain
    );
    assert!(
        hget.chain
            .iter()
            .all(|c| !c.resolution.contains("execute_command")),
        "{:?}",
        hget.chain
    );
    let _ = fs::remove_dir_all(&dir);
    let _ = fs::remove_dir_all(&cache);
}

#[test]
fn early_return_without_get_running_loop_provenance_still_reports() {
    // Attribute `_loop` / parameter `loop` must NOT suppress nested-event-loop.
    let dir = temp_project(&[(
        "app.py",
        concat!(
            "import asyncio\n",
            "\n",
            "class Runner:\n",
            "    def __init__(self):\n",
            "        self._loop = None\n",
            "\n",
            "    def start(self, coro):\n",
            "        if self._loop is not None:\n",
            "            return\n",
            "        self._loop = asyncio.new_event_loop()\n",
            "        self._loop.run_until_complete(coro)\n",
            "\n",
            "def run_param(coro, loop=None):\n",
            "    if loop is not None:\n",
            "        return loop.create_task(coro)\n",
            "    return asyncio.run(coro)\n",
            "\n",
            "async def handler_runner():\n",
            "    Runner().start(asyncio.sleep(0))\n",
            "\n",
            "async def handler_param():\n",
            "    run_param(asyncio.sleep(0))\n",
        ),
    )]);
    let report = scan_path(&dir).unwrap();
    assert!(
        report
            .findings
            .iter()
            .any(|f| { f.rule == "nested-event-loop" && f.entry.name == "handler_runner" }),
        "self._loop early-return must not guard: {:?}",
        report.findings
    );
    assert!(
        report
            .findings
            .iter()
            .any(|f| { f.rule == "nested-event-loop" && f.entry.name == "handler_param" }),
        "param loop early-return must not guard: {:?}",
        report.findings
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn unbounded_queue_param_shadow_and_name_rebind() {
    let dir = temp_project(&[(
        "q.py",
        concat!(
            "import queue\n",
            "\n",
            "_uq: \"queue.Queue[int]\" = queue.Queue()\n",
            "_bq: \"queue.Queue[int]\" = queue.Queue(maxsize=1)\n",
            "\n",
            "async def shadow_param(q: \"queue.Queue[int]\"):\n",
            "    q.put(1)\n",
            "\n",
            "async def rebind_name():\n",
            "    x = queue.Queue()\n",
            "    x = _bq\n",
            "    x.put(1)\n",
            "\n",
            "async def still_unbounded():\n",
            "    _uq.put(1)\n",
        ),
    )]);
    let report = scan_path(&dir).unwrap();
    let names: Vec<_> = report
        .findings
        .iter()
        .filter(|f| f.rule == "blocking-lock")
        .map(|f| f.entry.name.as_str())
        .collect();
    assert!(
        names.contains(&"shadow_param"),
        "param shadowing module unbounded queue must still report put: {names:?}"
    );
    assert!(
        names.contains(&"rebind_name"),
        "rebind to bounded queue must report put: {names:?}"
    );
    assert!(
        !names.contains(&"still_unbounded"),
        "module unbounded put must stay clean: {names:?}"
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn redis_lock_acquire_blocking_false_still_reports() {
    let dir = clone_fixture("sqla-redis-nonio");
    fs::write(
        dir.join("rlock.py"),
        concat!(
            "import redis\n",
            "\n",
            "async def use_redis_lock():\n",
            "    lk = redis.Redis().lock('x')\n",
            "    lk.acquire(blocking=False)\n",
            "\n",
            "async def factory_only():\n",
            "    redis.Redis().lock('x')\n",
        ),
    )
    .unwrap();
    let cache = fresh_cache_dir("redis-lock");
    let report = scan_path_with_cache(&dir, Some(&cache)).unwrap();
    assert!(
        report
            .findings
            .iter()
            .any(|f| f.entry.name == "use_redis_lock"),
        "redis Lock.acquire must not get threading nonblocking exemption: {:?}",
        report.findings
    );
    assert!(
        report
            .findings
            .iter()
            .all(|f| f.entry.name != "factory_only"),
        "Redis.lock factory must stay clean: {:?}",
        report.findings
    );
    let _ = fs::remove_dir_all(&dir);
    let _ = fs::remove_dir_all(&cache);
}

#[test]
fn sqlalchemy_bulk_and_query_iter_are_sinks() {
    let dir = clone_fixture("sqla-redis-nonio");
    fs::write(
        dir.join("bulk.py"),
        concat!(
            "from sqlalchemy.orm import Session\n",
            "\n",
            "class User:\n",
            "    pass\n",
            "\n",
            "async def use_bulk(s: Session):\n",
            "    s.bulk_save_objects([])\n",
            "\n",
            "async def use_query_iter(s: Session):\n",
            "    for u in s.query(User):\n",
            "        pass\n",
        ),
    )
    .unwrap();
    let cache = fresh_cache_dir("sqla-bulk");
    let report = scan_path_with_cache(&dir, Some(&cache)).unwrap();
    let bulk = report
        .findings
        .iter()
        .find(|f| f.entry.name == "use_bulk")
        .expect("bulk_save_objects must fire");
    assert!(
        bulk.chain
            .iter()
            .any(|c| c.resolution.contains("bulk_save_objects")),
        "{:?}",
        bulk.chain
    );
    let qiter = report
        .findings
        .iter()
        .find(|f| f.entry.name == "use_query_iter")
        .expect("Query.__iter__ must fire");
    assert!(
        qiter
            .chain
            .iter()
            .any(|c| { c.resolution.contains("Query.__iter__") || c.resolution.contains("Query") }),
        "{:?}",
        qiter.chain
    );
    let _ = fs::remove_dir_all(&dir);
    let _ = fs::remove_dir_all(&cache);
}

#[test]
fn self_referential_genexpr_reports_sleep_through_chain() {
    let dir = tempfile::tempdir().unwrap();
    // Previous binding must still be expanded when the name is reassigned.
    fs::write(
        dir.path().join("chain.py"),
        concat!(
            "import time\n",
            "async def f(r):\n",
            "    out = (time.sleep(1) for _ in r)\n",
            "    out = (x for x in out)\n",
            "    list(out)\n",
        ),
    )
    .unwrap();
    let report = scan_path(dir.path()).expect("scan must not abort");
    assert!(report.complete, "skipped={:?}", report.skipped);
    assert!(
        report.findings.iter().any(|f| f.rule == "blocking-sleep"),
        "self-reassigned genexpr chain must keep the sleep finding: {:?}",
        report.findings
    );
}

#[test]
fn self_referential_genexpr_does_not_stack_overflow() {
    let dir = clone_fixture("prerelease_stackoverflow");
    let report = scan_path(&dir.join("min.py")).expect("scan must not abort");
    assert!(report.complete, "skipped={:?}", report.skipped);
}

#[test]
fn self_referential_genexpr_chain_does_not_stack_overflow() {
    let dir = clone_fixture("prerelease_stackoverflow");
    let report = scan_path(&dir.join("a.py")).expect("scan must not abort");
    assert!(report.complete, "skipped={:?}", report.skipped);
}

#[test]
fn latin1_coding_cookie_is_analysed() {
    let dir = clone_fixture("prerelease_edge");
    let report = scan_path(&dir.join("pkg/latin1.py")).expect("scan");
    assert!(report.complete, "skipped={:?}", report.skipped);
    assert!(
        report.findings.iter().any(|f| f.rule == "blocking-sleep"),
        "latin-1 source with coding cookie must be analysed: {:?}",
        report.findings
    );
}

#[test]
fn encoding_param_annotation_is_not_a_coding_cookie() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("util.py"),
        concat!(
            "def to_string(s, encoding: str = \"utf-8\"):\n",
            "    return s\n",
            "\n",
            "import time\n",
            "async def h():\n",
            "    time.sleep(1)\n",
        ),
    )
    .unwrap();
    let report = scan_path(dir.path()).expect("scan");
    assert!(report.complete, "skipped={:?}", report.skipped);
    assert!(
        report.findings.iter().any(|f| f.rule == "blocking-sleep"),
        "UTF-8 file with `encoding:` param must not be skipped: {:?}",
        report.skipped
    );
}

#[test]
fn unknown_cookie_on_utf8_bytes_falls_back() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("gbk_cookie.py"),
        "# coding: gbk\nimport time\nasync def h():\n    time.sleep(1)\n",
    )
    .unwrap();
    let report = scan_path(dir.path()).expect("scan");
    assert!(report.complete, "skipped={:?}", report.skipped);
    assert!(report.findings.iter().any(|f| f.rule == "blocking-sleep"));
}

#[test]
fn undecodable_source_is_skipped() {
    let dir = tempfile::tempdir().unwrap();
    // Invalid UTF-8, no coding cookie → default UTF-8 fails.
    let path = dir.path().join("bad.py");
    fs::write(&path, b"s = \"\xe9\"\nasync def x():\n    pass\n").unwrap();
    let report = scan_path(&path).expect("scan");
    assert!(!report.complete);
    assert!(
        report.skipped.iter().any(|s| s.contains("bad.py")),
        "skipped={:?}",
        report.skipped
    );
}

#[test]
fn empty_directory_reports_no_python_files() {
    let dir = tempfile::tempdir().unwrap();
    let report = scan_path(dir.path()).expect("scan");
    assert!(report.complete);
    assert_ne!(
        report.environment.kind,
        async_doctor_core::EnvironmentKind::None,
        "cold empty scan must discover the environment (got {:?})",
        report.environment
    );
    assert_eq!(
        report.message.as_deref(),
        Some("No Python files were found.")
    );
    let json = report_json(dir.path());
    assert!(json.contains("No Python files were found."), "{json}");
}

#[test]
fn directory_without_py_reports_no_python_files() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("readme.txt"), "hi\n").unwrap();
    let report = scan_path(dir.path()).expect("scan");
    assert_eq!(
        report.message.as_deref(),
        Some("No Python files were found.")
    );
}

#[test]
fn per_file_panic_is_isolated() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("good.py"),
        "import time\nasync def ok():\n    time.sleep(1)\n",
    )
    .unwrap();
    fs::write(dir.path().join("__async_doctor_panic__.py"), "x = 1\n").unwrap();
    let report = scan_path(dir.path()).expect("scan must not abort");
    assert!(!report.complete);
    assert!(
        report
            .skipped
            .iter()
            .any(|s| s.contains("__async_doctor_panic__.py") && s.contains("panicked")),
        "skipped={:?}",
        report.skipped
    );
    assert!(
        report.findings.iter().any(|f| f.rule == "blocking-sleep"),
        "other files must still be analysed: {:?}",
        report.findings
    );
}

#[test]
fn p1_miss_probe_covers_stdlib_and_boto_gaps() {
    let dir = clone_fixture("miss-probe");
    let cache = fresh_cache_dir("miss-probe");
    let report = scan_path_with_cache(&dir, Some(&cache)).unwrap();
    let names: std::collections::BTreeSet<_> = report
        .findings
        .iter()
        .map(|f| f.entry.name.as_str())
        .collect();
    for expected in [
        "h_smtp", "h_ftp", "h_zip", "h_tar", "h_boto", "h_glob", "h_input", "h_future",
    ] {
        assert!(
            names.contains(expected),
            "missing {expected}: names={names:?} findings={:?}",
            report.findings
        );
    }
    let boto = report
        .findings
        .iter()
        .find(|f| f.entry.name == "h_boto")
        .unwrap();
    assert!(
        boto.fix.hint.contains("aioboto3") || boto.fix.hint.contains("aiobotocore"),
        "boto advice must mention aioboto3/aiobotocore: {}",
        boto.fix.hint
    );
    let fut = report
        .findings
        .iter()
        .find(|f| f.entry.name == "h_future")
        .unwrap();
    assert!(
        fut.chain
            .iter()
            .any(|c| c.resolution.contains("concurrent.futures.Future.result")),
        "{:?}",
        fut.chain
    );
    let _ = fs::remove_dir_all(&dir);
    let _ = fs::remove_dir_all(&cache);
}

#[test]
fn p1_boto2_client_and_resource_ops() {
    // Untyped stub: main cannot resolve dynamic get_object/get_item; this branch
    // must find them via syntactic boto receiver tracking (not merely client()).
    let dir = clone_fixture("boto2");
    let cache = fresh_cache_dir("boto2");
    let report = scan_path_with_cache(&dir, Some(&cache)).unwrap();
    let a_sinks: Vec<String> = report
        .findings
        .iter()
        .filter(|f| f.entry.name == "a")
        .flat_map(|f| {
            f.chain
                .iter()
                .flat_map(|c| [c.text.clone(), c.resolution.clone(), f.fix.hint.clone()])
        })
        .collect();
    assert!(
        a_sinks.iter().any(|s| s.contains("get_object")),
        "receiver-tracked get_object must fire (not just boto3.client): {a_sinks:?} {:?}",
        report.findings
    );
    let table_sinks: Vec<String> = report
        .findings
        .iter()
        .filter(|f| f.entry.name == "resource_table")
        .flat_map(|f| {
            f.chain
                .iter()
                .flat_map(|c| [c.text.clone(), c.resolution.clone()])
        })
        .collect();
    assert!(
        table_sinks.iter().any(|s| s.contains("get_item")),
        "receiver-tracked get_item must fire: {table_sinks:?} {:?}",
        report.findings
    );
    let _ = fs::remove_dir_all(&dir);
    let _ = fs::remove_dir_all(&cache);
}

#[test]
fn p1_aioboto3_session_client_not_sync_boto() {
    // aioboto3 + boto3 installed, no types-aioboto3: Session.client must not
    // synthesize botocore BaseClient ops (advice literally points at aioboto3).
    let dir = clone_fixture("aioboto3_ok");
    let cache = fresh_cache_dir("aioboto3_ok");
    let report = scan_path_with_cache(&dir, Some(&cache)).unwrap();
    let hits: Vec<_> = report
        .findings
        .iter()
        .filter(|f| f.entry.name == "aio_session_client" || f.entry.name == "aio_inline")
        .collect();
    assert!(
        hits.is_empty(),
        "aioboto3 Session.client / awaited receivers must be 0 findings: {hits:?} {:?}",
        report.findings
    );
    let _ = fs::remove_dir_all(&dir);
    let _ = fs::remove_dir_all(&cache);
}

#[test]
fn p1_chain_through_sink_helpers() {
    let dir = clone_fixture("chain-through-sinks");
    let report = scan_path(&dir).unwrap();
    assert!(
        report
            .findings
            .iter()
            .all(|f| f.entry.name == "handler" && f.rule == "blocking-file"),
        "{:?}",
        report.findings
    );
    assert!(
        report.findings.len() >= 2,
        "expected both _check and _patch paths: {:?}",
        report.findings
    );
    let joined: Vec<String> = report
        .findings
        .iter()
        .map(|f| {
            f.chain
                .iter()
                .map(|c| c.text.as_str())
                .collect::<Vec<_>>()
                .join(" | ")
        })
        .collect();
    assert!(
        joined
            .iter()
            .any(|c| c.contains("_check") && c.contains("open")),
        "missing _check path: {joined:?}"
    );
    assert!(
        joined
            .iter()
            .any(|c| c.contains("_patch") && c.contains("open")),
        "missing _patch path: {joined:?}"
    );
    // Paths must not mash _check and _patch into one chain.
    assert!(
        joined
            .iter()
            .all(|c| !(c.contains("_check") && c.contains("_patch"))),
        "cross-merged chains: {joined:?}"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn p1_genexpr_fanout_finishes_quickly() {
    let dir = clone_fixture("genexpr-fanout");
    let start = std::time::Instant::now();
    let report = scan_path(&dir).expect("scan must not abort");
    let elapsed = start.elapsed();
    assert!(report.complete, "skipped={:?}", report.skipped);
    assert!(
        report.findings.iter().any(|f| f.rule == "blocking-sleep"),
        "{:?}",
        report.findings
    );
    assert!(
        elapsed.as_secs() < 30,
        "fan-out expansion too slow: {elapsed:?}"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn p1_futures_map_wait_and_asyncio_precision() {
    let dir = clone_fixture("p1_futures_map");
    let report = scan_path(&dir).unwrap();
    let names: std::collections::BTreeSet<_> = report
        .findings
        .iter()
        .map(|f| f.entry.name.as_str())
        .collect();
    assert!(names.contains("map_consumed"), "{names:?}");
    assert!(names.contains("wait_blocked"), "{names:?}");
    assert!(
        !names.contains("submit_only_ok"),
        "bare submit must stay clean: {names:?}"
    );
    assert!(
        !names.contains("asyncio_future_result"),
        "asyncio.Future.result must not match concurrent.futures: {names:?} {:?}",
        report.findings
    );
    let _ = fs::remove_dir_all(&dir);
}
