//! Per-scan package-cache batch and warm materialization.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use async_doctor_core::{
    CallEdge, CallKind, CalleeRef, ConditionTag, EntryLabel, FunctionFact, FunctionId, LiteralArg,
    OffloadKind, ParamGuard, ProjectFacts,
};
use ruff_db::system::SystemPathBuf;

use crate::cache::{
    CachedEdge, CachedFollowTarget, CachedFunction, PackageCache, load_package_cache,
    save_package_cache,
};

use super::FollowTarget;

/// Internal test hook (debug builds only). Set via `Command::env` on child
/// scanners — never documented as user config. Inert when unset; compiled out
/// of the pause paths under `not(debug_assertions)`.
#[cfg(debug_assertions)]
const TEST_CACHE_RACE_ENV: &str = "__AD_TEST_CACHE_RACE";

#[cfg(debug_assertions)]
fn test_cache_race_enabled() -> bool {
    std::env::var_os(TEST_CACHE_RACE_ENV).is_some_and(|v| !v.is_empty())
}

/// After each on-disk package write, pause so a peer can observe a partial set.
#[cfg(debug_assertions)]
fn race_gap_after_package_write() {
    if !test_cache_race_enabled() {
        return;
    }
    std::thread::sleep(std::time::Duration::from_millis(40));
}

/// After a from-disk materialize sees unresolved cross-package probes, wait
/// until those package files exist on disk — but leave sticky misses in place.
/// Widens the miss→materialize window so a peer flush can land mid-check.
#[cfg(debug_assertions)]
fn race_hold_sticky_miss_until_peer_files(root: &Path, unresolved: &[(String, String, String)]) {
    if !test_cache_race_enabled() || unresolved.is_empty() {
        return;
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(90);
    for (pn, pv, pfp) in unresolved {
        while std::time::Instant::now() < deadline {
            if load_package_cache(root, pn, pv, pfp).is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        // Intentionally do not load: sticky miss must survive until reprobe /
        // extract fallback (or, without those, until findings are dropped).
    }
}

#[cfg(not(debug_assertions))]
fn race_gap_after_package_write() {}

#[cfg(not(debug_assertions))]
fn race_hold_sticky_miss_until_peer_files(_root: &Path, _unresolved: &[(String, String, String)]) {}

type PackageCacheKey = (String, String, String);

/// Per-scan package-cache index: each on-disk hit is parsed once and reused.
/// Negative probes are sticky for speed, but [`Self::reprobe`] clears a miss
/// when snapshot consistency would otherwise force an extract fallback (a peer
/// may have written the file mid-scan). Flush only packages dirtied by upserts.
pub struct PackageCacheBatch {
    root: PathBuf,
    /// (package, version, fingerprint) → cache loaded or created this scan.
    caches: BTreeMap<PackageCacheKey, PackageCache>,
    /// Keys whose current entry was populated from on-disk JSON (peer or prior
    /// run). In-progress upserts are not listed here.
    from_disk: BTreeSet<PackageCacheKey>,
    /// Negative disk probes remembered this scan (cleared by [`Self::reprobe`]).
    probed_miss: BTreeSet<PackageCacheKey>,
    /// Packages mutated via [`Self::upsert_function`] — only these are written.
    dirty: BTreeSet<PackageCacheKey>,
}

impl PackageCacheBatch {
    pub fn new(cache_root: PathBuf) -> Self {
        Self {
            root: cache_root,
            caches: BTreeMap::new(),
            from_disk: BTreeSet::new(),
            probed_miss: BTreeSet::new(),
            dirty: BTreeSet::new(),
        }
    }

    fn key(package: &str, version: &str, fingerprint: &str) -> PackageCacheKey {
        (
            package.to_string(),
            version.to_string(),
            fingerprint.to_string(),
        )
    }

    /// Load this package's on-disk cache into the per-scan index on first probe.
    ///
    /// Hits stay in [`Self::caches`]. Misses are sticky in [`Self::probed_miss`]
    /// so warm scans do not re-stat the same absent path every wave — call
    /// [`Self::reprobe`] before treating a miss as a snapshot inconsistency.
    pub fn ensure_loaded(&mut self, package: &str, version: &str, fingerprint: &str) {
        let key = Self::key(package, version, fingerprint);
        if self.caches.contains_key(&key) || self.probed_miss.contains(&key) {
            return;
        }
        if let Some(cache) = load_package_cache(&self.root, package, version, fingerprint) {
            self.caches.insert(key.clone(), cache);
            self.from_disk.insert(key);
        } else {
            self.probed_miss.insert(key);
        }
    }

    /// Clear a sticky miss and read disk again (peer may have created the file).
    pub fn reprobe(&mut self, package: &str, version: &str, fingerprint: &str) {
        let key = Self::key(package, version, fingerprint);
        if self.caches.contains_key(&key) {
            return;
        }
        self.probed_miss.remove(&key);
        self.ensure_loaded(package, version, fingerprint);
    }

    /// True when a compatible on-disk (or already-loaded) cache exists for this key.
    pub fn has(&self, package: &str, version: &str, fingerprint: &str) -> bool {
        self.caches
            .contains_key(&Self::key(package, version, fingerprint))
    }

    pub fn is_from_disk(&self, package: &str, version: &str, fingerprint: &str) -> bool {
        self.from_disk
            .contains(&Self::key(package, version, fingerprint))
    }

    pub fn get(&self, package: &str, version: &str, fingerprint: &str) -> Option<&PackageCache> {
        self.caches.get(&Self::key(package, version, fingerprint))
    }

    fn cache_mut(&mut self, package: &str, version: &str, fingerprint: &str) -> &mut PackageCache {
        let key = Self::key(package, version, fingerprint);
        self.dirty.insert(key.clone());
        self.probed_miss.remove(&key);
        if !self.caches.contains_key(&key) {
            match load_package_cache(&self.root, package, version, fingerprint) {
                Some(cache) => {
                    self.caches.insert(key.clone(), cache);
                    self.from_disk.insert(key.clone());
                }
                None => {
                    self.caches
                        .insert(key.clone(), PackageCache::new(package, version));
                }
            }
        }
        self.caches.get_mut(&key).expect("cache just inserted")
    }

    /// Record one analyzed third-party function under its External qualname key.
    #[allow(clippy::too_many_arguments)]
    pub fn upsert_function(
        &mut self,
        package: &str,
        version: &str,
        fingerprint: &str,
        external_key: &str,
        func: &FunctionFact,
        edges: &[CallEdge],
        id_to_external_key: &HashMap<FunctionId, String>,
        follow_targets: &HashMap<String, FollowTarget>,
    ) {
        let cache = self.cache_mut(package, version, fingerprint);
        let mut cached_edges: Vec<CachedEdge> = edges
            .iter()
            .filter(|e| e.caller == func.id)
            .map(|e| edge_to_cached(e, id_to_external_key))
            .collect();
        // Stable order for byte-identical cache files / warm materialization.
        cached_edges.sort_by(|a, b| {
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
        // Persist follow targets for External/third_party callees so warm runs
        // can continue cross-package waves without re-querying ty.
        for e in &cached_edges {
            if !matches!(e.callee_kind.as_str(), "external" | "third_party") {
                continue;
            }
            if let Some(t) = follow_targets.get(&e.callee_qualname) {
                cache.follow_targets.insert(
                    e.callee_qualname.clone(),
                    CachedFollowTarget {
                        qualname: t.qualname.clone(),
                        path: t.path.as_str().to_string(),
                        name_start: t.name_start,
                        package_name: t.package_name.clone(),
                        package_version: t.package_version.clone(),
                        package_fingerprint: t.package_fingerprint.clone(),
                    },
                );
            }
        }
        cache.functions.insert(
            external_key.to_string(),
            CachedFunction {
                key: external_key.to_string(),
                qualname: func.qualname.clone(),
                module: func.module.clone(),
                line: func.line,
                is_async: func.is_async,
                is_generator: func.is_generator,
                param_names: func.param_names.clone(),
                invokes_params: func.invokes_params.clone(),
                is_offload_wrapper: func.is_offload_wrapper,
                edges: cached_edges,
            },
        );
    }

    /// Atomically write every dirty package cache once.
    ///
    /// Only persist follow_targets for callees that were actually analyzed this
    /// run (present in some package's `functions`). Otherwise warm reinjection
    /// would follow budget-skipped targets that cold never extracted → extra
    /// site-packages candidates and cold≠warm.
    ///
    /// `analyzed` spans every loaded package (read or write) so cross-package
    /// follow_targets stay valid when the callee was only materialized.
    pub fn flush(&self) {
        if self.dirty.is_empty() {
            return;
        }
        let analyzed: BTreeSet<String> = self
            .caches
            .values()
            .flat_map(|c| c.functions.keys().cloned())
            .collect();
        for key in &self.dirty {
            let Some(cache) = self.caches.get(key) else {
                continue;
            };
            let mut to_save = cache.clone();
            to_save.follow_targets.retain(|q, _| analyzed.contains(q));
            let _ = save_package_cache(&self.root, &to_save, &key.2);
            race_gap_after_package_write();
        }
    }
}

fn edge_to_cached(e: &CallEdge, id_to_external_key: &HashMap<FunctionId, String>) -> CachedEdge {
    let (callee_kind, callee_qualname, offload_kind, param_invoke_index) = match &e.callee {
        CalleeRef::External { qualname } => ("external".to_string(), qualname.clone(), None, None),
        CalleeRef::Offload(k) => (
            "offload".to_string(),
            k.as_str().to_string(),
            Some(k.as_str().to_string()),
            None,
        ),
        CalleeRef::Unresolved { reason } => ("unresolved".to_string(), reason.clone(), None, None),
        CalleeRef::Project(id) => {
            let key = id_to_external_key
                .get(id)
                .cloned()
                .unwrap_or_else(|| format!("unresolved:project:{}", id.0));
            let kind = if id_to_external_key.contains_key(id) {
                "third_party"
            } else {
                "unresolved"
            };
            (kind.to_string(), key, None, None)
        }
        CalleeRef::ParamInvoke { index } => (
            "param_invoke".to_string(),
            format!("param:{index}"),
            None,
            Some(*index),
        ),
    };
    let positional_literals = e
        .positional_literals
        .iter()
        .map(|lit| {
            lit.map(|l| match l {
                LiteralArg::True => "true".to_string(),
                LiteralArg::False => "false".to_string(),
                LiteralArg::None => "none".to_string(),
            })
        })
        .collect();
    let positional_callable_quals = e
        .positional_callables
        .iter()
        .map(|opt| opt.and_then(|fid| id_to_external_key.get(&fid).cloned()))
        .collect();
    let keyword_callable_quals = e
        .keyword_callables
        .iter()
        .filter_map(|(k, fid)| id_to_external_key.get(fid).cloned().map(|q| (k.clone(), q)))
        .collect();
    CachedEdge {
        callee_qualname,
        callee_kind,
        offload_kind,
        param_invoke_index,
        is_offload_wrapper: e.is_offload_wrapper,
        kind: call_kind_str(e.kind).to_string(),
        file: e.file.clone(),
        line: e.line,
        text: e.text.clone(),
        conditions: e
            .conditions
            .iter()
            .map(|c| c.as_str().to_string())
            .collect(),
        occurrence: e.occurrence,
        positional_literals,
        positional_callable_quals,
        keyword_callable_quals,
        param_guard_index: e.param_guard.map(|g| g.index),
        param_guard_inverted: e.param_guard.map(|g| g.inverted),
        is_awaited: e.is_awaited,
        callee_is_coroutine: e.callee_is_coroutine,
        // Eager offload is a first-party call-site property; third-party cache
        // never records it (serde default false on read).
    }
}

fn call_kind_str(kind: CallKind) -> &'static str {
    match kind {
        CallKind::Call => "Call",
        CallKind::WithEnter => "WithEnter",
        CallKind::WithExit => "WithExit",
        CallKind::ForIter => "ForIter",
        CallKind::PropertyGet => "PropertyGet",
        CallKind::GeneratorConsume => "GeneratorConsume",
    }
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
        _ => None,
    }
}

/// Materialize cached package functions that are requested into `facts`.
///
/// Keys in `wanted` must be External qualnames (same as cache keys).
/// Returns the set of keys that were materialized from cache.
///
/// Only the explicitly requested keys are loaded — no transitive closure.
/// Nested callees are followed by later budgeted waves (same as cold extract),
/// so warm runs cannot bypass [`crate::extract::FOLLOW_BUDGET_DEFAULT`].
///
/// Reads go through [`PackageCacheBatch`] so each package JSON is parsed once
/// per scan, not once per follow wave.
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
    cache_batch.ensure_loaded(package, version, fingerprint);
    if !cache_batch.has(package, version, fingerprint) {
        return Vec::new();
    }

    // Probe cross-package follow_target caches (misses are retried — a peer may
    // have written them since an earlier probe in this scan).
    let cross_probes: Vec<(String, String, String, String)> = {
        let cache = cache_batch
            .get(package, version, fingerprint)
            .expect("has() true");
        cache
            .follow_targets
            .iter()
            .filter_map(|(q, t)| {
                if cache.functions.contains_key(q) {
                    return None;
                }
                match (&t.package_name, &t.package_version, &t.package_fingerprint) {
                    (Some(pn), Some(pv), Some(pfp)) => {
                        Some((pn.clone(), pv.clone(), pfp.clone(), q.clone()))
                    }
                    _ => None,
                }
            })
            .collect()
    };
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
    // Debug-only test hook (before reprobe): wait until peer files exist while
    // keeping sticky misses, so CI debug builds reproduce the race reliably.
    if cache_batch.is_from_disk(package, version, fingerprint) && !unresolved.is_empty() {
        race_hold_sticky_miss_until_peer_files(&cache_batch.root, &unresolved);
    }
    // Snapshot consistency: re-probe misses once; if still incomplete, fall
    // back to ty extract — cache hit/miss may change speed, never results.
    // Only enforce for on-disk hits. In-progress upserts naturally list
    // follow_targets for not-yet-analyzed callees and must not force extract.
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

    // Re-inject follow targets that were analyzed (in this cache's functions, or
    // loadable from the target's own package cache). On-disk entries are
    // flush-filtered to analyzed-only.
    for (q, t) in &cache.follow_targets {
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
    // Create FunctionFacts for wanted keys present in cache (no transitive extras).
    let mut pending_edges: Vec<(FunctionId, Vec<CachedEdge>)> = Vec::new();
    for q in wanted {
        if qual_to_id.contains_key(q) {
            continue;
        }
        let Some(cf) = cache.functions.get(q) else {
            continue;
        };
        let id = FunctionId(facts.functions.len() as u32);
        // Prefer a real path recorded on an edge (`pkg/__init__.py`) over the
        // invented `{module}.py` form, which mis-labels package __init__ modules.
        let fact_file = cf
            .edges
            .iter()
            .find(|e| !e.file.is_empty())
            .map(|e| e.file.clone())
            .unwrap_or_else(|| format!("site-packages/{}.py", cf.module.replace('.', "/")));
        facts.functions.push(FunctionFact {
            id,
            file: fact_file,
            line: cf.line,
            module: cf.module.clone(),
            qualname: cf.qualname.clone(),
            is_async: cf.is_async,
            is_generator: cf.is_generator,
            label: EntryLabel::Other,
            is_sync_route: false,
            async_twins: Vec::new(),
            param_names: cf.param_names.clone(),
            is_third_party: true,
            package_name: Some(package.to_string()),
            package_version: Some(version.to_string()),
            invokes_params: cf.invokes_params.clone(),
            is_offload_wrapper: cf.is_offload_wrapper,
        });
        qual_to_id.insert(q.clone(), id);
        id_to_external_key.insert(id, q.clone());
        pending_edges.push((id, cf.edges.clone()));
        loaded.push(q.clone());
    }

    // Emit edges with full summaries, resolving third_party → Project when the
    // callee was also in `wanted` (and thus already assigned an id). Nested
    // callees stay External for a later budgeted wave — matching cold extract.
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
    for (caller, edges) in pending_edges {
        let mut edges = edges;
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
            let positional_callables = edge
                .positional_callable_quals
                .iter()
                .map(|o| o.as_ref().and_then(|q| qual_to_id.get(q).copied()))
                .collect();
            let param_guard = match (edge.param_guard_index, edge.param_guard_inverted) {
                (Some(index), Some(inverted)) => Some(ParamGuard { index, inverted }),
                _ => None,
            };
            let conditions: Vec<ConditionTag> = edge
                .conditions
                .iter()
                .filter_map(|s| parse_condition(s))
                .collect();
            let edge_file = if edge.file.is_empty() {
                let mod_path = facts.functions[caller.index()].module.replace('.', "/");
                format!("site-packages/{mod_path}.py")
            } else {
                edge.file.clone()
            };
            // Keyword callables in the cache are External quals; higher-order
            // kwargs across packages are rare — drop unresolved ones.
            let keyword_callables = edge
                .keyword_callable_quals
                .iter()
                .filter_map(|(k, q)| qual_to_id.get(q).map(|id| (k.clone(), *id)))
                .collect();
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
                positional_callables,
                // First-party offload externals are not stored in the package
                // cache; third-party offload targets are rare for advice.
                positional_externals: vec![],
                positional_constructed_receivers: vec![],
                keyword_callables,
                param_guard,
                is_awaited: edge.is_awaited,
                callee_is_coroutine: edge.callee_is_coroutine,
                is_eager_offload_arg: false,
                context_from_caller: None,
                constructed_receiver: None,
            });
        }
    }
    loaded.sort();
    loaded
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
                // Leave as External so a later follow wave / sink table can
                // resolve it; never invent project:N.
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
