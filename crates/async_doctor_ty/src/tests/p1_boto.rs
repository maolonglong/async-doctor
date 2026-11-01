//! P1 boto3 / aioboto3 recall: receivers, stubs roots, factory finding ids.

use std::fs;

use super::helpers::{clone_fixture, fresh_cache_dir};
use crate::scan_path_with_cache;

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
fn p1_boto_factory_ids_distinct_for_session_and_client() {
    // boto3.client and Session().client share sink key `boto3.client`; occurrence
    // must be counted on that key so both call sites keep distinct finding ids.
    let dir = clone_fixture("boto_factory_ids");
    let cache = fresh_cache_dir("boto_factory_ids");
    let report = scan_path_with_cache(&dir, Some(&cache)).unwrap();
    for entry in [
        "e1_client_then_session",
        "e1_session_then_client",
        "e1_bound_session",
    ] {
        let hits: Vec<_> = report
            .findings
            .iter()
            .filter(|f| f.entry.name == entry)
            .collect();
        assert_eq!(
            hits.len(),
            2,
            "{entry} must keep both factory sites: {hits:?} {:?}",
            report.findings
        );
        assert_ne!(hits[0].id, hits[1].id, "{entry} ids must differ");
        let texts: Vec<_> = hits
            .iter()
            .flat_map(|f| f.chain.iter().map(|c| c.text.as_str()))
            .collect();
        assert!(
            texts.iter().any(|t| t.contains("boto3.client")),
            "{entry} missing boto3.client: {texts:?}"
        );
        assert!(
            texts
                .iter()
                .any(|t| t.contains("Session") || t.contains("sess.client")),
            "{entry} missing Session/sess.client: {texts:?}"
        );
    }
    assert!(
        report
            .findings
            .iter()
            .all(|f| f.entry.name != "available_regions_ok"),
        "get_available_regions must stay clean: {:?}",
        report.findings
    );
    let _ = fs::remove_dir_all(&dir);
    let _ = fs::remove_dir_all(&cache);
}

#[test]
fn p1_boto3_stubs_session_client_ops_and_aioboto_clean() {
    // boto3-stubs-shaped layout: Session().client receivers must flag AWS ops;
    // aioboto3 (+ aioboto3-stubs) must stay at 0.
    let dir = clone_fixture("boto3_stubs_session");
    let cache = fresh_cache_dir("boto3_stubs_session");
    let report = scan_path_with_cache(&dir, Some(&cache)).unwrap();
    let get_sinks: Vec<String> = report
        .findings
        .iter()
        .filter(|f| f.entry.name == "stubs_session_client_get_object")
        .flat_map(|f| {
            f.chain
                .iter()
                .flat_map(|c| [c.text.clone(), c.resolution.clone()])
        })
        .collect();
    assert!(
        get_sinks.iter().any(|s| s.contains("get_object")),
        "Session().client receiver get_object must fire: {get_sinks:?} {:?}",
        report.findings
    );
    let put_sinks: Vec<String> = report
        .findings
        .iter()
        .filter(|f| f.entry.name == "stubs_bound_session_put_object")
        .flat_map(|f| {
            f.chain
                .iter()
                .flat_map(|c| [c.text.clone(), c.resolution.clone()])
        })
        .collect();
    assert!(
        put_sinks.iter().any(|s| s.contains("put_object")),
        "sess.client receiver put_object must fire: {put_sinks:?} {:?}",
        report.findings
    );
    let aio_hits: Vec<_> = report
        .findings
        .iter()
        .filter(|f| f.entry.name == "aio_session_client_ok" || f.entry.name == "aio_inline_ok")
        .collect();
    assert!(
        aio_hits.is_empty(),
        "aioboto3 must stay clean with stubs present: {aio_hits:?} {:?}",
        report.findings
    );
    let _ = fs::remove_dir_all(&dir);
    let _ = fs::remove_dir_all(&cache);
}

fn assert_session_internals_clean_and_client_ops_fire(fixture: &str) {
    let dir = clone_fixture(fixture);
    let cache = fresh_cache_dir(fixture);
    let report = scan_path_with_cache(&dir, Some(&cache)).unwrap();
    for entry in ["regions_ok", "service_ops_ok"] {
        let hits: Vec<_> = report
            .findings
            .iter()
            .filter(|f| f.entry.name == entry)
            .collect();
        assert!(
            hits.is_empty(),
            "{fixture}/{entry} must be 0 blocking-http (session internals): {hits:?} {:?}",
            report.findings
        );
    }
    let get_sinks: Vec<String> = report
        .findings
        .iter()
        .filter(|f| f.entry.name == "client_get_object_blocks")
        .flat_map(|f| {
            f.chain
                .iter()
                .flat_map(|c| [c.text.clone(), c.resolution.clone()])
        })
        .collect();
    assert!(
        get_sinks.iter().any(|s| s.contains("get_object")),
        "{fixture} client.get_object must fire: {get_sinks:?} {:?}",
        report.findings
    );
    let describe_sinks: Vec<String> = report
        .findings
        .iter()
        .filter(|f| f.entry.name == "session_client_describe_blocks")
        .flat_map(|f| {
            f.chain
                .iter()
                .flat_map(|c| [c.text.clone(), c.resolution.clone()])
        })
        .collect();
    assert!(
        describe_sinks
            .iter()
            .any(|s| s.contains("describe_db_instances")),
        "{fixture} Session().client(...).describe_db_instances must fire: {describe_sinks:?} {:?}",
        report.findings
    );
    let _ = fs::remove_dir_all(&dir);
    let _ = fs::remove_dir_all(&cache);
}

#[test]
fn p1_boto_session_internals_clean_without_stubs() {
    // Real-shaped boto3/botocore (no boto3-stubs): get_available_regions /
    // get_session().get_service_model must not flag via session get_* heuristics.
    assert_session_internals_clean_and_client_ops_fire("boto_session_internals");
}

#[test]
fn p1_boto_session_internals_clean_with_stubs() {
    assert_session_internals_clean_and_client_ops_fire("boto_session_internals_stubs");
}
