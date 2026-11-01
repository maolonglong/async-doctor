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
