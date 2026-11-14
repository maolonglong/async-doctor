//! Warm materialization of cached third-party functions into [`ProjectFacts`].

use std::collections::{BTreeMap, BTreeSet, HashMap};

use async_doctor_core::{
    CallEdge, CallKind, CalleeRef, ConditionTag, EntryLabel, FunctionFact, FunctionId, LiteralArg,
    OffloadKind, ParamGuard, ProjectFacts,
};
use ruff_db::system::SystemPathBuf;

use crate::cache::{CachedEdge, PackageCache};

use super::super::FollowTarget;
use super::{PackageCacheBatch, for_each_edge_target_qual, race_hold_sticky_miss_until_peer_files};

/// Cross-package / later-wave higher-order callable arguments recorded on a
/// materialized edge, resolved after all packages in the wave materialize.
#[derive(Debug, Clone)]
pub(crate) struct DeferredCallablePatch {
    pub edge_index: usize,
    pub positional_quals: Vec<Option<String>>,
    pub keyword_quals: Vec<(String, String)>,
}

/// Materialize cached package functions that are requested into `facts`.
#[cfg(test)]
#[allow(clippy::too_many_arguments)]
pub(crate) fn materialize_cached_functions(
    facts: &mut ProjectFacts,
    cache_batch: &mut PackageCacheBatch,
    package: &str,
    version: &str,
    fingerprint: &str,
    wanted: &[String],
    qual_to_id: &mut BTreeMap<String, FunctionId>,
    id_to_external_key: &mut HashMap<FunctionId, String>,
    follow_targets: &mut HashMap<String, FollowTarget>,
) -> Vec<String> {
    let mut unused = Vec::new();
    materialize_cached_functions_with_deferred(
        facts,
        cache_batch,
        package,
        version,
        fingerprint,
        wanted,
        qual_to_id,
        id_to_external_key,
        follow_targets,
        &mut unused,
    )
}

/// Materialize cached package functions while recording deferred callable quals
/// so cross-package callbacks resolve regardless of package alphabetical order.
#[allow(clippy::too_many_arguments)]
pub(crate) fn materialize_cached_functions_with_deferred(
    facts: &mut ProjectFacts,
    cache_batch: &mut PackageCacheBatch,
    package: &str,
    version: &str,
    fingerprint: &str,
    wanted: &[String],
    qual_to_id: &mut BTreeMap<String, FunctionId>,
    id_to_external_key: &mut HashMap<FunctionId, String>,
    follow_targets: &mut HashMap<String, FollowTarget>,
    deferred_callables: &mut Vec<DeferredCallablePatch>,
) -> Vec<String> {
    cache_batch.ensure_loaded(package, version, fingerprint);
    if !cache_batch.has(package, version, fingerprint) {
        return Vec::new();
    }

    let (reachable_quals, cross_probes, has_unprobed_missing_target) = {
        let cache = cache_batch
            .get(package, version, fingerprint)
            .expect("has() true");
        let reachable = reachable_quals_from_wanted(cache, wanted);
        let mut probes = Vec::new();
        let mut missing_no_meta = false;
        for (q, t) in &cache.follow_targets {
            if !reachable.contains(q) || cache.functions.contains_key(q) {
                continue;
            }
            match (&t.package_name, &t.package_version, &t.package_fingerprint) {
                (Some(pn), Some(pv), Some(pfp)) => {
                    probes.push((pn.clone(), pv.clone(), pfp.clone(), q.clone()));
                }
                _ => {
                    missing_no_meta = true;
                }
            }
        }
        (reachable, probes, missing_no_meta)
    };
    if cache_batch.is_from_disk(package, version, fingerprint) && has_unprobed_missing_target {
        return Vec::new();
    }
    for (pn, pv, pfp, _) in &cross_probes {
        cache_batch.ensure_loaded(pn, pv, pfp);
    }
    let mut loadable_cross: BTreeSet<String> = cross_probes
        .iter()
        .filter(|(pn, pv, pfp, q)| {
            cache_batch
                .get(pn, pv, pfp)
                .is_some_and(|c| c.functions.contains_key(q))
        })
        .map(|(_, _, _, q)| q.clone())
        .collect();

    let unresolved: Vec<(String, String, String)> = cross_probes
        .iter()
        .filter(|(_, _, _, q)| !loadable_cross.contains(q))
        .map(|(pn, pv, pfp, _)| (pn.clone(), pv.clone(), pfp.clone()))
        .collect();
    if cache_batch.is_from_disk(package, version, fingerprint) && !unresolved.is_empty() {
        race_hold_sticky_miss_until_peer_files(&cache_batch.root, &unresolved);
    }
    if cache_batch.is_from_disk(package, version, fingerprint) && !unresolved.is_empty() {
        let mut seen_reprobe: BTreeSet<(String, String, String)> = BTreeSet::new();
        for (pn, pv, pfp) in &unresolved {
            if seen_reprobe.insert((pn.clone(), pv.clone(), pfp.clone())) {
                cache_batch.reprobe(pn, pv, pfp);
            }
        }
        loadable_cross = cross_probes
            .iter()
            .filter(|(pn, pv, pfp, q)| {
                cache_batch
                    .get(pn, pv, pfp)
                    .is_some_and(|c| c.functions.contains_key(q))
            })
            .map(|(_, _, _, q)| q.clone())
            .collect();
        if cross_probes
            .iter()
            .any(|(_, _, _, q)| !loadable_cross.contains(q))
        {
            return Vec::new();
        }
    }

    let cache = cache_batch
        .get(package, version, fingerprint)
        .expect("has() true");

    for (q, t) in &cache.follow_targets {
        if !reachable_quals.contains(q) {
            continue;
        }
        if !cache.functions.contains_key(q) && !loadable_cross.contains(q) {
            continue;
        }
        follow_targets
            .entry(q.clone())
            .or_insert_with(|| FollowTarget {
                qualname: t.qualname.clone(),
                path: SystemPathBuf::from(t.path.as_str()),
                name_start: t.name_start,
                package_name: t.package_name.clone(),
                package_version: t.package_version.clone(),
                package_fingerprint: t.package_fingerprint.clone(),
            });
    }

    let mut loaded = Vec::new();
    let mut pending_edges: Vec<(FunctionId, Vec<CachedEdge>)> = Vec::new();
    for q in wanted {
        if qual_to_id.contains_key(q) {
            continue;
        }
        let Some(cf) = cache.functions.get(q) else {
            continue;
        };
        let id = FunctionId(facts.functions.len() as u32);
        let fact_file = if !cf.file.is_empty() {
            cf.file.clone()
        } else {
            cf.edges
                .iter()
                .find(|e| !e.file.is_empty())
                .map(|e| e.file.clone())
                .unwrap_or_else(|| format!("site-packages/{}.py", cf.module.replace('.', "/")))
        };
        facts.functions.push(FunctionFact {
            id,
            file: fact_file,
            line: cf.line,
            end_line: cf.line,
            module: cf.module.clone(),
            qualname: cf.qualname.clone(),
            is_async: cf.is_async,
            is_generator: cf.is_generator,
            label: EntryLabel::Other,
            is_sync_route: false,
            param_names: cf.param_names.clone(),
            is_third_party: true,
            package_name: Some(package.to_string()),
            package_version: Some(version.to_string()),
            invokes_params: cf.invokes_params.clone(),
            is_offload_wrapper: cf.is_offload_wrapper,
            offload_decorator: None,
        });
        facts.decorator_refs.entry(id).or_default().extend(
            cf.decorator_quals
                .iter()
                .map(|q| async_doctor_core::DecoratorRef::External(q.clone())),
        );
        qual_to_id.insert(q.clone(), id);
        id_to_external_key.insert(id, q.clone());
        pending_edges.push((id, cf.edges.clone()));
        loaded.push(q.clone());
    }

    pending_edges.sort_by(|a, b| {
        let ka = id_to_external_key
            .get(&a.0)
            .map(String::as_str)
            .unwrap_or("");
        let kb = id_to_external_key
            .get(&b.0)
            .map(String::as_str)
            .unwrap_or("");
        ka.cmp(kb)
    });
    for (caller, mut edges) in pending_edges {
        edges.sort_by(|a, b| {
            (
                a.line,
                a.occurrence,
                a.callee_qualname.as_str(),
                a.text.as_str(),
            )
                .cmp(&(
                    b.line,
                    b.occurrence,
                    b.callee_qualname.as_str(),
                    b.text.as_str(),
                ))
        });
        for edge in edges {
            let callee = cached_callee_to_ref(&edge, qual_to_id);
            let positional_literals = edge
                .positional_literals
                .iter()
                .map(|o| o.as_deref().and_then(parse_literal))
                .collect();
            let keyword_literals = edge
                .keyword_literals
                .iter()
                .map(|(k, o)| (k.clone(), o.as_deref().and_then(parse_literal)))
                .collect();
            let positional_callables = edge
                .positional_callable_quals
                .iter()
                .map(|o| o.as_ref().and_then(|q| qual_to_id.get(q).copied()))
                .collect();
            let param_guards = edge
                .param_guards
                .iter()
                .map(|&(index, inverted)| ParamGuard { index, inverted })
                .collect();
            let conditions: Vec<ConditionTag> = edge
                .conditions
                .iter()
                .filter_map(|s| parse_condition(s))
                .collect();
            let edge_file = if edge.file.is_empty() {
                facts.functions[caller.index()].file.clone()
            } else {
                edge.file.clone()
            };
            let keyword_callables = edge
                .keyword_callable_quals
                .iter()
                .filter_map(|(k, q)| qual_to_id.get(q).map(|id| (k.clone(), *id)))
                .collect();
            let edge_index = facts.edges.len();
            if edge.positional_callable_quals.iter().any(Option::is_some)
                || !edge.keyword_callable_quals.is_empty()
            {
                deferred_callables.push(DeferredCallablePatch {
                    edge_index,
                    positional_quals: edge.positional_callable_quals.clone(),
                    keyword_quals: edge.keyword_callable_quals.clone(),
                });
            }
            facts.edges.push(CallEdge {
                caller,
                callee,
                file: edge_file,
                line: edge.line,
                text: edge.text.clone(),
                kind: parse_call_kind(&edge.kind),
                conditions,
                occurrence: edge.occurrence,
                is_offload_wrapper: edge.is_offload_wrapper,
                positional_literals,
                keyword_literals,
                positional_callables,
                positional_externals: edge.positional_externals.clone(),
                positional_constructed_receivers: edge.positional_constructed_receivers.clone(),
                keyword_callables,
                param_guards,
                is_awaited: edge.is_awaited,
                callee_is_coroutine: edge.callee_is_coroutine,
                is_eager_offload_arg: edge.is_eager_offload_arg,
                is_bound_call: edge.is_bound_call,
                context_from_caller: None,
                context_from_edge: None,
                constructed_receiver: edge.constructed_receiver.clone(),
            });
        }
    }
    loaded.sort();
    loaded
}

fn reachable_quals_from_wanted(cache: &PackageCache, wanted: &[String]) -> BTreeSet<String> {
    let mut visited_fns: BTreeSet<String> = BTreeSet::new();
    let mut queue: Vec<String> = Vec::new();
    for q in wanted {
        if cache.functions.contains_key(q) && visited_fns.insert(q.clone()) {
            queue.push(q.clone());
        }
    }
    let mut reachable: BTreeSet<String> = BTreeSet::new();
    while let Some(fn_key) = queue.pop() {
        let Some(cf) = cache.functions.get(&fn_key) else {
            continue;
        };
        for edge in &cf.edges {
            for_each_edge_target_qual(edge, |target_q| {
                reachable.insert(target_q.to_string());
                if cache.functions.contains_key(target_q)
                    && visited_fns.insert(target_q.to_string())
                {
                    queue.push(target_q.to_string());
                }
            });
        }
    }
    reachable
}

fn parse_call_kind(s: &str) -> CallKind {
    match s {
        "WithEnter" => CallKind::WithEnter,
        "WithExit" => CallKind::WithExit,
        "ForIter" => CallKind::ForIter,
        "PropertyGet" => CallKind::PropertyGet,
        "GeneratorConsume" => CallKind::GeneratorConsume,
        _ => CallKind::Call,
    }
}

fn parse_condition(s: &str) -> Option<ConditionTag> {
    match s {
        "unconditional" => Some(ConditionTag::Unconditional),
        "all-paths" => Some(ConditionTag::AllPaths),
        "loop" => Some(ConditionTag::Loop),
        "async-branch" => Some(ConditionTag::AsyncBranch),
        "cache_miss_dependent" => Some(ConditionTag::CacheMissDependent),
        "no_running_loop_guard" => Some(ConditionTag::NoRunningLoopGuard),
        "sync_entry" => Some(ConditionTag::SyncEntry),
        _ => None,
    }
}

fn parse_literal(s: &str) -> Option<LiteralArg> {
    match s {
        "true" => Some(LiteralArg::True),
        "false" => Some(LiteralArg::False),
        "none" => Some(LiteralArg::None),
        _ => s
            .strip_prefix("param:")?
            .parse()
            .ok()
            .map(LiteralArg::Param),
    }
}

fn cached_callee_to_ref(edge: &CachedEdge, qual_to_id: &BTreeMap<String, FunctionId>) -> CalleeRef {
    match edge.callee_kind.as_str() {
        "offload" => {
            let kind = match edge.offload_kind.as_deref() {
                Some("asyncio.to_thread") => OffloadKind::AsyncioToThread,
                Some("loop.run_in_executor") => OffloadKind::RunInExecutor,
                Some("anyio.to_thread.run_sync") => OffloadKind::AnyioToThread,
                Some("starlette.concurrency.run_in_threadpool") => OffloadKind::StarletteThreadpool,
                Some("concurrent.futures.ThreadPoolExecutor") => OffloadKind::ThreadPoolExecutor,
                Some("asgiref.sync.sync_to_async") => OffloadKind::AsgirefSyncToAsync,
                Some("channels.db.database_sync_to_async") => OffloadKind::ChannelsDbSyncToAsync,
                _ => OffloadKind::AsyncioToThread,
            };
            CalleeRef::Offload(kind)
        }
        "unresolved" => CalleeRef::Unresolved {
            reason: edge.callee_qualname.clone(),
        },
        "param_invoke" => CalleeRef::ParamInvoke {
            index: edge.param_invoke_index.unwrap_or(0),
        },
        "third_party" | "external" => {
            if let Some(id) = qual_to_id.get(&edge.callee_qualname) {
                CalleeRef::Project(*id)
            } else {
                CalleeRef::External {
                    qualname: edge.callee_qualname.clone(),
                }
            }
        }
        _ => CalleeRef::External {
            qualname: edge.callee_qualname.clone(),
        },
    }
}
