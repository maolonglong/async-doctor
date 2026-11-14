//! Per-scan package-cache batch and warm materialization.

mod materialize;

#[cfg(test)]
pub(crate) use materialize::materialize_cached_functions;
pub(crate) use materialize::{DeferredCallablePatch, materialize_cached_functions_with_deferred};

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use async_doctor_core::{CallEdge, CallKind, CalleeRef, FunctionFact, FunctionId, LiteralArg};

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
pub(super) fn race_hold_sticky_miss_until_peer_files(
    root: &Path,
    unresolved: &[(String, String, String)],
) {
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
    }
}

#[cfg(not(debug_assertions))]
fn race_gap_after_package_write() {}

#[cfg(not(debug_assertions))]
pub(super) fn race_hold_sticky_miss_until_peer_files(
    _root: &Path,
    _unresolved: &[(String, String, String)],
) {
}

pub(super) fn for_each_edge_target_qual<'a>(edge: &'a CachedEdge, mut f: impl FnMut(&'a str)) {
    if matches!(edge.callee_kind.as_str(), "external" | "third_party") {
        f(&edge.callee_qualname);
    }
    for q in edge.positional_callable_quals.iter().flatten() {
        f(q);
    }
    for (_, q) in &edge.keyword_callable_quals {
        f(q);
    }
    for q in edge.positional_externals.iter().flatten() {
        f(q);
    }
}

type PackageCacheKey = (String, String, String);

/// Per-scan package-cache index: each on-disk hit is parsed once and reused.
/// Negative probes are sticky for speed, but [`Self::reprobe`] clears a miss
/// when snapshot consistency would otherwise force an extract fallback (a peer
/// may have written the file mid-scan). Flush only packages dirtied by upserts.
pub struct PackageCacheBatch {
    pub(super) root: PathBuf,
    caches: BTreeMap<PackageCacheKey, PackageCache>,
    from_disk: BTreeSet<PackageCacheKey>,
    probed_miss: BTreeSet<PackageCacheKey>,
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
        decorator_quals: Vec<String>,
    ) {
        let cache = self.cache_mut(package, version, fingerprint);
        let mut cached_edges: Vec<CachedEdge> = edges
            .iter()
            .filter(|e| e.caller == func.id)
            .map(|e| edge_to_cached(e, id_to_external_key))
            .collect();
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
        for e in &cached_edges {
            for_each_edge_target_qual(e, |qual| {
                if let Some(t) = follow_targets.get(qual) {
                    cache.follow_targets.insert(
                        qual.to_string(),
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
            });
        }
        cache.functions.insert(
            external_key.to_string(),
            CachedFunction {
                key: external_key.to_string(),
                qualname: func.qualname.clone(),
                module: func.module.clone(),
                file: func.file.clone(),
                line: func.line,
                is_async: func.is_async,
                is_generator: func.is_generator,
                param_names: func.param_names.clone(),
                invokes_params: func.invokes_params.clone(),
                is_offload_wrapper: func.is_offload_wrapper,
                decorator_quals,
                edges: cached_edges,
            },
        );
    }

    /// Atomically write every dirty package cache once, unioning any compatible
    /// functions concurrently flushed by a peer scanner before pruning unreferenced
    /// `follow_targets`.
    pub fn flush(&self) {
        if self.dirty.is_empty() {
            return;
        }
        for key in &self.dirty {
            let Some(cache) = self.caches.get(key) else {
                continue;
            };
            let mut to_save = cache.clone();
            if let Some(disk) = load_package_cache(&self.root, &key.0, &key.1, &key.2) {
                for (fk, fv) in disk.functions {
                    to_save.functions.entry(fk).or_insert(fv);
                }
                for (tk, tv) in disk.follow_targets {
                    to_save.follow_targets.entry(tk).or_insert(tv);
                }
            }
            let mut referenced: BTreeSet<String> = BTreeSet::new();
            for f in to_save.functions.values() {
                for e in &f.edges {
                    for_each_edge_target_qual(e, |q| {
                        referenced.insert(q.to_string());
                    });
                }
            }
            to_save.follow_targets.retain(|q, _| referenced.contains(q));
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
        .map(|lit| lit.map(literal_str))
        .collect();
    let keyword_literals = e
        .keyword_literals
        .iter()
        .map(|(k, lit)| (k.clone(), lit.map(literal_str)))
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
    let param_guards = e
        .param_guards
        .iter()
        .map(|g| (g.index, g.inverted))
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
        keyword_literals,
        positional_callable_quals,
        positional_externals: e.positional_externals.clone(),
        positional_constructed_receivers: e.positional_constructed_receivers.clone(),
        keyword_callable_quals,
        param_guards,
        is_awaited: e.is_awaited,
        callee_is_coroutine: e.callee_is_coroutine,
        is_eager_offload_arg: e.is_eager_offload_arg,
        is_bound_call: e.is_bound_call,
        constructed_receiver: e.constructed_receiver.clone(),
    }
}

fn literal_str(lit: LiteralArg) -> String {
    match lit {
        LiteralArg::True => "true".to_string(),
        LiteralArg::False => "false".to_string(),
        LiteralArg::None => "none".to_string(),
        LiteralArg::Param(index) => format!("param:{index}"),
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
