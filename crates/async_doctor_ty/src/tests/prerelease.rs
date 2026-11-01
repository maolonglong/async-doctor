//! Pre-release precision regressions (SQLAlchemy / redis / locks / loop guards).

use std::fs;

use super::helpers::{clone_fixture, fresh_cache_dir, temp_project};
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
