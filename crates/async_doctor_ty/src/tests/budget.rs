//! Cold/warm follow-budget determinism.

use std::collections::{BTreeMap, HashMap};
use std::fs;

use async_doctor_core::ProjectFacts;

use super::helpers::{clone_fixture, fresh_cache_dir, report_json_with_cache_budget};
use crate::FOLLOW_BUDGET_DEFAULT;
use crate::cache::{
    CachedEdge, CachedFollowTarget, CachedFunction, PackageCache, save_package_cache,
};
use crate::sitepkgs::materialize_cached_functions;

/// Tiny budget: `a0` free, then `a1`/`a2` consume it — never enters `libb.gate`.
const TINY_BUDGET: usize = 2;

#[test]
fn m2_follow_budget_cold_warm_json_identical_when_truncated() {
    let dir = clone_fixture("m2_follow_budget_truncation");
    let cache = fresh_cache_dir("budget-trunc");

    let cold = report_json_with_cache_budget(&dir, &cache, TINY_BUDGET);
    let warm = report_json_with_cache_budget(&dir, &cache, TINY_BUDGET);
    let warm2 = report_json_with_cache_budget(&dir, &cache, TINY_BUDGET);

    assert_eq!(
        cold, warm,
        "cold vs warm must be byte-identical under a tiny follow budget\ncold={cold}\nwarm={warm}"
    );
    assert_eq!(
        warm, warm2,
        "warm vs warm2 must match\nwarm={warm}\nwarm2={warm2}"
    );

    let cold_r: async_doctor_core::Report = serde_json::from_str(&cold).unwrap();
    let warm_r: async_doctor_core::Report = serde_json::from_str(&warm).unwrap();
    assert_eq!(cold_r.findings.len(), warm_r.findings.len());
    assert_eq!(cold_r.candidates.len(), warm_r.candidates.len());

    // Truncation must keep libb.gate / time.sleep out of reach on both runs.
    // (Buggy warm transitively materialize'd a0..a2 for free, kept a full
    // budget, and continued into libb while cold had already truncated.)
    let sleep_hit = |r: &async_doctor_core::Report| {
        r.findings.iter().any(|f| f.rule == "blocking-sleep")
            || r.candidates.iter().any(|c| c.rule == "blocking-sleep")
    };
    assert!(
        !sleep_hit(&cold_r),
        "tiny budget must truncate before libb sleep; findings={:?} candidates={:?}",
        cold_r.findings,
        cold_r.candidates
    );

    let _ = fs::remove_dir_all(&dir);
    let _ = fs::remove_dir_all(&cache);
}

/// Populate cache with a full-budget run, then compare a tiny-budget cold
/// (empty cache) to a tiny-budget warm (reusing the deep cache).
///
/// On main, warm transitively materialize'd the deep cached chain for free and
/// still had budget left to enter `libb` — cold with the same tiny budget could
/// not. After the fix both truncate identically.
#[test]
fn m2_follow_budget_warm_must_not_exceed_cold_via_deeper_cache() {
    let dir = clone_fixture("m2_follow_budget_truncation");

    // Seed: full budget reaches libb.gate / time.sleep and writes the cache.
    let deep_cache = fresh_cache_dir("budget-deep-seed");
    let seeded = report_json_with_cache_budget(&dir, &deep_cache, FOLLOW_BUDGET_DEFAULT);
    let seeded_r: async_doctor_core::Report = serde_json::from_str(&seeded).unwrap();
    assert!(
        seeded_r
            .findings
            .iter()
            .any(|f| f.rule == "blocking-sleep"),
        "seed run must reach sleep so the cache holds the deep chain: {:?}",
        seeded_r.findings
    );

    // Warm with tiny budget against the deep cache.
    let warm_tiny = report_json_with_cache_budget(&dir, &deep_cache, TINY_BUDGET);

    // Cold with the same tiny budget against an empty cache.
    let empty = fresh_cache_dir("budget-tiny-cold");
    let cold_tiny = report_json_with_cache_budget(&dir, &empty, TINY_BUDGET);

    assert_eq!(
        cold_tiny, warm_tiny,
        "tiny-budget cold (empty cache) must match tiny-budget warm (deep cache)\n\
         cold={cold_tiny}\nwarm={warm_tiny}"
    );

    let cold_r: async_doctor_core::Report = serde_json::from_str(&cold_tiny).unwrap();
    assert!(
        !cold_r
            .findings
            .iter()
            .any(|f| f.rule == "blocking-sleep")
            && !cold_r
                .candidates
                .iter()
                .any(|c| c.rule == "blocking-sleep"),
        "tiny budget must not reach sleep: {:?}",
        cold_r
    );

    let _ = fs::remove_dir_all(&dir);
    let _ = fs::remove_dir_all(&deep_cache);
    let _ = fs::remove_dir_all(&empty);
}

#[test]
fn m2_follow_budget_default_reaches_deep_sinks() {
    let dir = clone_fixture("m2_follow_budget_truncation");
    let cache = fresh_cache_dir("budget-full");

    let cold = report_json_with_cache_budget(&dir, &cache, FOLLOW_BUDGET_DEFAULT);
    let warm = report_json_with_cache_budget(&dir, &cache, FOLLOW_BUDGET_DEFAULT);
    assert_eq!(
        cold, warm,
        "default-budget cold/warm must match\ncold={cold}\nwarm={warm}"
    );

    let report: async_doctor_core::Report = serde_json::from_str(&cold).unwrap();
    assert!(
        report
            .findings
            .iter()
            .any(|f| f.rule == "blocking-sleep" && f.entry.name == "handler"),
        "default budget must reach time.sleep via libb.gate: {:?}",
        report.findings
    );

    let _ = fs::remove_dir_all(&dir);
    let _ = fs::remove_dir_all(&cache);
}

/// Direct regression for free transitive cache materialization (the warm
/// under-charge). Must fail on main where `materialize_cached_functions`
/// closed over cached callees without a budget ticket.
#[test]
fn materialize_cached_functions_loads_only_wanted_keys() {
    let root = fresh_cache_dir("mat-only-wanted");
    let fp = "fpbudgettest";
    let mut cache = PackageCache::new("liba", "1.0.0");
    cache.functions.insert(
        "liba.core.a0".into(),
        CachedFunction {
            key: "liba.core.a0".into(),
            qualname: "a0".into(),
            module: "liba.core".into(),
            line: 1,
            is_async: false,
            is_generator: false,
            param_names: vec![],
            invokes_params: vec![],
            is_offload_wrapper: false,
            edges: vec![CachedEdge {
                callee_qualname: "liba.core.a1".into(),
                callee_kind: "third_party".into(),
                offload_kind: None,
                param_invoke_index: None,
                is_offload_wrapper: false,
                kind: "Call".into(),
                file: "site-packages/liba/core.py".into(),
                line: 2,
                text: "a1()".into(),
                conditions: vec!["unconditional".into()],
                occurrence: 0,
                positional_literals: vec![],
                positional_callable_quals: vec![],
                param_guard_index: None,
                param_guard_inverted: None,
                is_awaited: false,
            }],
        },
    );
    cache.functions.insert(
        "liba.core.a1".into(),
        CachedFunction {
            key: "liba.core.a1".into(),
            qualname: "a1".into(),
            module: "liba.core".into(),
            line: 5,
            is_async: false,
            is_generator: false,
            param_names: vec![],
            invokes_params: vec![],
            is_offload_wrapper: false,
            edges: vec![CachedEdge {
                callee_qualname: "time.sleep".into(),
                callee_kind: "external".into(),
                offload_kind: None,
                param_invoke_index: None,
                is_offload_wrapper: false,
                kind: "Call".into(),
                file: "site-packages/liba/core.py".into(),
                line: 6,
                text: "time.sleep(1)".into(),
                conditions: vec!["unconditional".into()],
                occurrence: 0,
                positional_literals: vec![],
                positional_callable_quals: vec![],
                param_guard_index: None,
                param_guard_inverted: None,
                is_awaited: false,
            }],
        },
    );
    cache.follow_targets.insert(
        "liba.core.a1".into(),
        CachedFollowTarget {
            qualname: "liba.core.a1".into(),
            path: "/tmp/site-packages/liba/core.py".into(),
            name_start: 0,
            package_name: Some("liba".into()),
            package_version: Some("1.0.0".into()),
            package_fingerprint: Some(fp.into()),
        },
    );
    save_package_cache(&root, &cache, fp).unwrap();

    let mut facts = ProjectFacts::default();
    let mut qual_to_id = BTreeMap::new();
    let mut id_to_external_key = HashMap::new();
    let mut follow_targets = HashMap::new();
    let loaded = materialize_cached_functions(
        &mut facts,
        &root,
        "liba",
        "1.0.0",
        fp,
        &["liba.core.a0".into()],
        &mut qual_to_id,
        &mut id_to_external_key,
        &mut follow_targets,
    );
    assert_eq!(
        loaded,
        vec!["liba.core.a0".to_string()],
        "must not transitively materialize a1 when only a0 was wanted (warm budget bypass)"
    );
    assert!(
        qual_to_id.contains_key("liba.core.a0"),
        "a0 should be loaded"
    );
    assert!(
        !qual_to_id.contains_key("liba.core.a1"),
        "a1 must wait for a budgeted follow wave, got keys {:?}",
        qual_to_id.keys().collect::<Vec<_>>()
    );
    // Edge to a1 must remain External so a later wave can charge budget for it.
    assert!(
        facts.edges.iter().any(|e| {
            matches!(
                &e.callee,
                async_doctor_core::CalleeRef::External { qualname } if qualname == "liba.core.a1"
            )
        }),
        "a0→a1 should stay External until a budgeted wave; edges={:?}",
        facts.edges
    );

    let _ = fs::remove_dir_all(&root);
}
