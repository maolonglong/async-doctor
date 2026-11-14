//! Follow reachable third-party site-packages callees into the call graph.

mod extract_fn;

#[cfg(test)]
pub(crate) use extract_fn::{reset_third_party_file_parse_count, third_party_file_parse_count};

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use async_doctor_core::{CalleeRef, DecoratorRef, FunctionId, ProjectFacts, SinkTable};
use async_doctor_rules::{BuiltinSinkTable, is_async_counterpart};
use ruff_db::system::SystemPathBuf;

use crate::cache::effective_cache_root;
use crate::db::AsyncDoctorDb;
use crate::sitepkgs::{
    DeferredCallablePatch, FollowTarget, PackageCacheBatch, PackageMetaMemo,
    materialize_cached_functions_with_deferred,
};

use super::ctx::DefKey;
use extract_fn::extract_third_party_file_group;

/// Build an adjacency index: caller → edge indices. O(E) once, O(degree) lookup after.
pub(crate) fn edges_by_caller(facts: &ProjectFacts) -> Vec<Vec<usize>> {
    let mut idx = vec![Vec::new(); facts.functions.len()];
    for (i, edge) in facts.edges.iter().enumerate() {
        let c = edge.caller.index();
        if c < idx.len() {
            idx[c].push(i);
        }
    }
    idx
}

/// Append newly added edges (and newly allocated function slots) into the index.
pub(crate) fn extend_edges_by_caller(
    facts: &ProjectFacts,
    idx: &mut Vec<Vec<usize>>,
    from_edge: usize,
) {
    if idx.len() < facts.functions.len() {
        idx.resize(facts.functions.len(), Vec::new());
    }
    for i in from_edge..facts.edges.len() {
        let c = facts.edges[i].caller.index();
        if c < idx.len() {
            idx[c].push(i);
        }
    }
}

/// Default budget for following calls into third-party code after the first wave.
pub(crate) const DEFAULT_FOLLOW_BUDGET: usize = 256;

const FOLLOW_DB_RESET_INTERVAL: usize = 64;

/// After first-party extraction, follow External callees into site-packages `.py` sources.
#[allow(clippy::too_many_arguments)]
pub(crate) fn follow_third_party(
    db: &mut AsyncDoctorDb,
    facts: &mut ProjectFacts,
    def_map: &mut HashMap<DefKey, FunctionId>,
    qual_map: &mut HashMap<(String, String), FunctionId>,
    follow_targets: &mut HashMap<String, FollowTarget>,
    package_meta: &mut PackageMetaMemo,
    followable_extra_roots: &[PathBuf],
    cache_dir: Option<&Path>,
    follow_budget: usize,
) {
    let mut uncached_since_reset = 0usize;
    let sinks = BuiltinSinkTable::new();
    let mut qual_to_id: BTreeMap<String, FunctionId> = BTreeMap::new();
    let mut id_to_external_key: HashMap<FunctionId, String> = HashMap::new();
    let mut deferred_callables: Vec<DeferredCallablePatch> = Vec::new();
    let mut cache_batch = PackageCacheBatch::new(effective_cache_root(cache_dir));
    let mut by_caller = edges_by_caller(facts);

    let mut reachable_fns: BTreeSet<FunctionId> = BTreeSet::new();
    let mut work: BTreeSet<FunctionId> = facts
        .functions
        .iter()
        .filter(|f| f.is_async && !f.is_sync_route)
        .map(|f| f.id)
        .collect();
    reachable_fns.extend(work.iter().copied());

    let mut pending_externals: BTreeSet<String> = BTreeSet::new();
    while let Some(&fid) = work.iter().next() {
        work.remove(&fid);
        let Some(edge_ids) = by_caller.get(fid.index()) else {
            continue;
        };
        for &ei in edge_ids {
            let edge = &facts.edges[ei];
            if edge.is_offload_wrapper {
                continue;
            }
            match &edge.callee {
                CalleeRef::Project(id) => {
                    if reachable_fns.insert(*id) {
                        work.insert(*id);
                    }
                }
                CalleeRef::External { qualname }
                    if !is_async_counterpart(qualname) && sinks.lookup(qualname).is_none() =>
                {
                    pending_externals.insert(qualname.clone());
                }
                _ => {}
            }
        }
    }

    let mut by_package: BTreeMap<(String, String, String), Vec<String>> = BTreeMap::new();
    let mut no_meta: Vec<String> = Vec::new();
    for q in &pending_externals {
        let Some(target) = follow_targets.get(q) else {
            continue;
        };
        match (
            &target.package_name,
            &target.package_version,
            &target.package_fingerprint,
        ) {
            (Some(n), Some(v), Some(fp)) => {
                by_package
                    .entry((n.clone(), v.clone(), fp.clone()))
                    .or_default()
                    .push(q.clone());
            }
            _ => no_meta.push(q.clone()),
        }
    }
    for wanted in by_package.values_mut() {
        wanted.sort();
        wanted.dedup();
    }
    no_meta.sort();
    no_meta.dedup();

    for ((pkg, ver, fp), wanted) in &by_package {
        let edge_before = facts.edges.len();
        let loaded = materialize_cached_functions_with_deferred(
            facts,
            &mut cache_batch,
            pkg,
            ver,
            fp,
            wanted,
            &mut qual_to_id,
            &mut id_to_external_key,
            follow_targets,
            &mut deferred_callables,
        );
        extend_edges_by_caller(facts, &mut by_caller, edge_before);
        for q in &loaded {
            if let Some(id) = qual_to_id.get(q) {
                reachable_fns.insert(*id);
            }
        }
    }

    let wave0_uncached = by_package
        .values()
        .flat_map(|w| w.iter().cloned())
        .chain(no_meta);
    let wave0_groups = group_uncached_by_file(wave0_uncached, &qual_to_id, follow_targets);
    let _ = run_file_groups(
        db,
        facts,
        def_map,
        qual_map,
        &mut qual_to_id,
        &mut id_to_external_key,
        follow_targets,
        package_meta,
        followable_extra_roots,
        &mut cache_batch,
        &mut by_caller,
        &mut reachable_fns,
        &mut uncached_since_reset,
        &wave0_groups,
    );

    rewrite_external_to_project(facts, &qual_to_id, &sinks, &deferred_callables);

    let mut followed: usize = 0;
    let mut skipped_by_budget: BTreeSet<String> = BTreeSet::new();
    loop {
        let mut wave_candidates: BTreeMap<String, u32> = BTreeMap::new();
        for &fid in &reachable_fns {
            let Some(edge_ids) = by_caller.get(fid.index()) else {
                continue;
            };
            for &ei in edge_ids {
                let edge = &facts.edges[ei];
                if edge.is_offload_wrapper {
                    continue;
                }
                let CalleeRef::External { qualname } = &edge.callee else {
                    continue;
                };
                if is_followable_candidate(qualname, &qual_to_id, follow_targets, &sinks) {
                    *wave_candidates.entry(qualname.clone()).or_insert(0) += 1;
                }
            }
        }
        for patch in &deferred_callables {
            let Some(edge) = facts.edges.get(patch.edge_index) else {
                continue;
            };
            if !reachable_fns.contains(&edge.caller) {
                continue;
            }
            for q in patch
                .positional_quals
                .iter()
                .flatten()
                .chain(patch.keyword_quals.iter().map(|(_, q)| q))
            {
                if is_followable_candidate(q, &qual_to_id, follow_targets, &sinks) {
                    *wave_candidates.entry(q.clone()).or_insert(0) += 1;
                }
            }
        }
        if wave_candidates.is_empty() {
            break;
        }
        let mut ranked: Vec<(u32, String)> =
            wave_candidates.into_iter().map(|(q, n)| (n, q)).collect();
        ranked.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
        let remaining = follow_budget.saturating_sub(followed);
        if remaining == 0 {
            for (_, q) in ranked {
                skipped_by_budget.insert(q);
            }
            break;
        }
        if ranked.len() > remaining {
            for (_, q) in &ranked[remaining..] {
                skipped_by_budget.insert(q.clone());
            }
            ranked.truncate(remaining);
        }

        let mut progressed = false;
        let mut wave_by_pkg: BTreeMap<(String, String, String), Vec<String>> = BTreeMap::new();
        for (_count, q) in &ranked {
            let Some(target) = follow_targets.get(q) else {
                continue;
            };
            if let (Some(n), Some(v), Some(fp)) = (
                &target.package_name,
                &target.package_version,
                &target.package_fingerprint,
            ) {
                wave_by_pkg
                    .entry((n.clone(), v.clone(), fp.clone()))
                    .or_default()
                    .push(q.clone());
            }
        }
        for ((pkg, ver, fp), wanted) in &wave_by_pkg {
            let edge_before = facts.edges.len();
            let loaded = materialize_cached_functions_with_deferred(
                facts,
                &mut cache_batch,
                pkg,
                ver,
                fp,
                wanted,
                &mut qual_to_id,
                &mut id_to_external_key,
                follow_targets,
                &mut deferred_callables,
            );
            if !loaded.is_empty() {
                progressed = true;
                extend_edges_by_caller(facts, &mut by_caller, edge_before);
                for q in &loaded {
                    if let Some(id) = qual_to_id.get(q) {
                        reachable_fns.insert(*id);
                    }
                    followed += 1;
                }
            }
        }

        let wave_groups = group_uncached_by_file(
            ranked.into_iter().map(|(_, q)| q),
            &qual_to_id,
            follow_targets,
        );
        let extracted_count = run_file_groups(
            db,
            facts,
            def_map,
            qual_map,
            &mut qual_to_id,
            &mut id_to_external_key,
            follow_targets,
            package_meta,
            followable_extra_roots,
            &mut cache_batch,
            &mut by_caller,
            &mut reachable_fns,
            &mut uncached_since_reset,
            &wave_groups,
        );
        if extracted_count > 0 {
            followed += extracted_count;
            progressed = true;
        }

        rewrite_external_to_project(facts, &qual_to_id, &sinks, &deferred_callables);
        if !progressed {
            break;
        }
    }

    cache_batch.flush();
    let budget_skipped = skipped_by_budget
        .iter()
        .filter(|q| !qual_to_id.contains_key(*q))
        .count();
    facts.environment.follow_budget_skipped = budget_skipped as u32;
}

fn is_followable_candidate(
    qualname: &str,
    qual_to_id: &BTreeMap<String, FunctionId>,
    follow_targets: &HashMap<String, FollowTarget>,
    sinks: &BuiltinSinkTable,
) -> bool {
    !is_async_counterpart(qualname)
        && sinks.lookup(qualname).is_none()
        && !qual_to_id.contains_key(qualname)
        && follow_targets.contains_key(qualname)
}

fn group_uncached_by_file(
    quals: impl IntoIterator<Item = String>,
    qual_to_id: &BTreeMap<String, FunctionId>,
    follow_targets: &HashMap<String, FollowTarget>,
) -> Vec<Vec<(String, FollowTarget)>> {
    let mut file_order: Vec<SystemPathBuf> = Vec::new();
    let mut by_file: HashMap<SystemPathBuf, Vec<(String, FollowTarget)>> = HashMap::new();
    let mut seen_quals: BTreeSet<String> = BTreeSet::new();
    for q in quals {
        if qual_to_id.contains_key(&q) || !seen_quals.insert(q.clone()) {
            continue;
        }
        let Some(target) = follow_targets.get(&q).cloned() else {
            continue;
        };
        let path = target.path.clone();
        by_file
            .entry(path.clone())
            .or_insert_with(|| {
                file_order.push(path);
                Vec::new()
            })
            .push((q, target));
    }
    file_order
        .into_iter()
        .filter_map(|p| by_file.remove(&p))
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn run_file_groups(
    db: &mut AsyncDoctorDb,
    facts: &mut ProjectFacts,
    def_map: &mut HashMap<DefKey, FunctionId>,
    qual_map: &mut HashMap<(String, String), FunctionId>,
    qual_to_id: &mut BTreeMap<String, FunctionId>,
    id_to_external_key: &mut HashMap<FunctionId, String>,
    follow_targets: &mut HashMap<String, FollowTarget>,
    package_meta: &mut PackageMetaMemo,
    followable_extra_roots: &[PathBuf],
    cache_batch: &mut PackageCacheBatch,
    by_caller: &mut Vec<Vec<usize>>,
    reachable_fns: &mut BTreeSet<FunctionId>,
    uncached_since_reset: &mut usize,
    groups: &[Vec<(String, FollowTarget)>],
) -> usize {
    let mut total_extracted = 0usize;
    for group in groups {
        if *uncached_since_reset >= FOLLOW_DB_RESET_INTERVAL {
            *db = db.fresh_empty();
            *uncached_since_reset = 0;
        }
        let group_edge_before = facts.edges.len();
        let extracted = extract_third_party_file_group(
            db,
            facts,
            def_map,
            qual_map,
            qual_to_id,
            id_to_external_key,
            follow_targets,
            package_meta,
            followable_extra_roots,
            group,
        );
        if extracted.is_empty() {
            continue;
        }
        extend_edges_by_caller(facts, by_caller, group_edge_before);
        for item in extracted {
            reachable_fns.insert(item.id);
            total_extracted += 1;
            if item.newly_extracted {
                *uncached_since_reset += 1;
            }
            if let (Some(pn), Some(pv), Some(pfp)) = (
                facts.functions[item.id.index()].package_name.clone(),
                facts.functions[item.id.index()].package_version.clone(),
                item.target.package_fingerprint.clone(),
            ) {
                let edges_slice = match item.edge_range {
                    Some((start, end)) => &facts.edges[start..end],
                    None => &facts.edges[..],
                };
                cache_batch.upsert_function(
                    &pn,
                    &pv,
                    &pfp,
                    &item.qualname,
                    &facts.functions[item.id.index()],
                    edges_slice,
                    id_to_external_key,
                    follow_targets,
                    decorator_quals_for(facts, item.id),
                );
            }
        }
    }
    total_extracted
}

/// Rewrite External→Project for extracted callees (skipping known sinks) and
/// patch deferred higher-order callable quals materialized across packages/waves.
pub(crate) fn rewrite_external_to_project(
    facts: &mut ProjectFacts,
    qual_to_id: &BTreeMap<String, FunctionId>,
    sinks: &BuiltinSinkTable,
    deferred_callables: &[DeferredCallablePatch],
) {
    for edge in &mut facts.edges {
        let CalleeRef::External { qualname } = &edge.callee else {
            continue;
        };
        if sinks.lookup(qualname).is_some() {
            continue;
        }
        if let Some(id) = qual_to_id.get(qualname) {
            edge.callee = CalleeRef::Project(*id);
        }
    }
    for patch in deferred_callables {
        let Some(edge) = facts.edges.get_mut(patch.edge_index) else {
            continue;
        };
        for (slot, maybe_q) in edge
            .positional_callables
            .iter_mut()
            .zip(&patch.positional_quals)
        {
            if slot.is_none()
                && let Some(q) = maybe_q
                && let Some(&fid) = qual_to_id.get(q)
            {
                *slot = Some(fid);
            }
        }
        if !patch.keyword_quals.is_empty() {
            edge.keyword_callables = patch
                .keyword_quals
                .iter()
                .filter_map(|(k, q)| qual_to_id.get(q).map(|&id| (k.clone(), id)))
                .collect();
        }
    }
}

fn decorator_quals_for(facts: &ProjectFacts, id: FunctionId) -> Vec<String> {
    facts
        .decorator_refs
        .get(&id)
        .map(|refs| {
            refs.iter()
                .map(|r| match r {
                    DecoratorRef::External(q) => q.clone(),
                    DecoratorRef::Function(dfid) => {
                        let f = &facts.functions[dfid.index()];
                        if f.module.is_empty() {
                            f.qualname.clone()
                        } else {
                            format!("{}.{}", f.module, f.qualname)
                        }
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}
