//! Site-packages helpers: dist-info metadata, follow targets, and package cache I/O.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use async_doctor_core::{
    CallEdge, CallKind, CalleeRef, ConditionTag, EntryLabel, FunctionFact, FunctionId, LiteralArg,
    OffloadKind, ParamGuard, ProjectFacts,
};
use ruff_db::system::SystemPathBuf;

use crate::cache::{
    CachedEdge, CachedFollowTarget, CachedFunction, PackageCache, load_package_cache,
    package_content_fingerprint, path_str_is_third_party_python, save_package_cache,
};

/// A third-party `.py` definition we may extract on demand.
#[derive(Debug, Clone)]
pub struct FollowTarget {
    pub qualname: String,
    pub path: SystemPathBuf,
    pub name_start: u32,
    pub package_name: Option<String>,
    pub package_version: Option<String>,
    /// Content fingerprint for cache invalidation (RECORD / py mtimes).
    pub package_fingerprint: Option<String>,
}

/// `(site-/dist-packages root, top-level import name)`.
type PackageMetaKey = (String, String);
/// Cached `(name, version, fingerprint)` (or absence).
type PackageMetaValue = Option<(String, String, String)>;

/// Process-local memo for package meta lookups.
///
/// `package_meta_for_path` used to re-`read_dir` + fingerprint on every call site;
/// one large venv made that dominate wall/sys time. Memoize once per scan.
static PACKAGE_META_MEMO: Mutex<Option<HashMap<PackageMetaKey, PackageMetaValue>>> =
    Mutex::new(None);

/// Clear the package-meta memo at the start of each scan (and in tests).
pub fn clear_package_meta_memo() {
    if let Ok(mut g) = PACKAGE_META_MEMO.lock() {
        *g = None;
    }
}

/// Parse `Name:` / `Version:` from a dist-info `METADATA` file.
pub fn parse_dist_info_metadata(text: &str) -> Option<(String, String)> {
    let mut name = None;
    let mut version = None;
    for line in text.lines() {
        if line.is_empty() {
            if name.is_some() {
                break;
            }
            continue;
        }
        if let Some(rest) = line.strip_prefix("Name:") {
            name = Some(rest.trim().to_string());
        } else if let Some(rest) = line.strip_prefix("Version:") {
            version = Some(rest.trim().to_string());
        }
        if name.is_some() && version.is_some() {
            break;
        }
    }
    Some((name?, version?))
}

/// Locate neighboring `*.dist-info` for a file under site-/dist-packages.
/// Returns `(name, version, content_fingerprint)`.
pub fn package_meta_for_path(py_path: &Path) -> Option<(String, String, String)> {
    let s = py_path.to_string_lossy().replace('\\', "/");
    let (root, rest) = match (s.find("/site-packages/"), s.find("/dist-packages/")) {
        (Some(idx), _) => (
            PathBuf::from(&s[..idx + "/site-packages".len()]),
            &s[idx + "/site-packages/".len()..],
        ),
        (None, Some(idx)) => (
            PathBuf::from(&s[..idx + "/dist-packages".len()]),
            &s[idx + "/dist-packages/".len()..],
        ),
        (None, None) => return None,
    };
    let top = rest.split('/').next()?.to_string();
    if top.is_empty() {
        return None;
    }

    let key = (root.to_string_lossy().to_string(), top.clone());
    {
        let mut guard = PACKAGE_META_MEMO.lock().ok()?;
        let memo = guard.get_or_insert_with(HashMap::new);
        if let Some(cached) = memo.get(&key) {
            return cached.clone();
        }
    }

    let computed = package_meta_uncached(&root, &top);
    if let Ok(mut guard) = PACKAGE_META_MEMO.lock() {
        let memo = guard.get_or_insert_with(HashMap::new);
        memo.insert(key, computed.clone());
    }
    computed
}

fn package_meta_uncached(root: &Path, top: &str) -> Option<(String, String, String)> {
    let mut entries: Vec<_> = fs::read_dir(root).ok()?.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    let top_norm = normalize_dist_name(top);
    for entry in entries {
        let fname = entry.file_name();
        let fname = fname.to_string_lossy();
        if !fname.ends_with(".dist-info") {
            continue;
        }
        let stem = fname.trim_end_matches(".dist-info");
        let dist_name = stem.split('-').next().unwrap_or(stem);
        if normalize_dist_name(dist_name) != top_norm {
            continue;
        }
        let dist_info = entry.path();
        let meta_path = dist_info.join("METADATA");
        let text = fs::read_to_string(meta_path).ok()?;
        if let Some((name, ver)) = parse_dist_info_metadata(&text) {
            let fp = package_content_fingerprint(&dist_info);
            return Some((name, ver, fp));
        }
    }
    let pkg_dir = root.join(top);
    let fp = package_content_fingerprint(&pkg_dir);
    Some((top.to_string(), "unknown".to_string(), fp))
}

fn normalize_dist_name(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c == '_' || c == '.' {
                '-'
            } else {
                c.to_ascii_lowercase()
            }
        })
        .collect()
}

/// True when a system path string is third-party Python source under
/// site-packages or dist-packages (both are followed; never treated as entries).
pub fn is_followable_site_packages_py(path_str: &str) -> bool {
    let s = path_str.replace('\\', "/");
    path_str_is_third_party_python(&s) && s.ends_with(".py") && !s.ends_with(".pyi")
}

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
        param_guard_index: e.param_guard.map(|g| g.index),
        param_guard_inverted: e.param_guard.map(|g| g.inverted),
        is_awaited: e.is_awaited,
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

    // Snapshot consistency: if any cross-package follow_target from this cache
    // entry is not loadable, a peer may have written a partial set of package
    // files (or we sticky-missed before the peer finished). Re-probe misses
    // once; if still incomplete, fall back to ty extract — cache hit/miss may
    // change speed, never results.
    // Only enforce for on-disk hits. In-progress upserts naturally list
    // follow_targets for not-yet-analyzed callees and must not force extract.
    if cache_batch.is_from_disk(package, version, fingerprint)
        && cross_probes
            .iter()
            .any(|(_, _, _, q)| !loadable_cross.contains(q))
    {
        let mut seen_reprobe: BTreeSet<(String, String, String)> = BTreeSet::new();
        for (pn, pv, pfp, q) in &cross_probes {
            if loadable_cross.contains(q) {
                continue;
            }
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
        facts.functions.push(FunctionFact {
            id,
            file: format!("site-packages/{}.py", cf.module.replace('.', "/")),
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
                param_guard,
                is_awaited: edge.is_awaited,
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
