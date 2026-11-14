//! On-disk cache for third-party (site-packages) analysis results.
//!
//! Layout:
//! `{cache_root}/v{schema}/{tool}/{package}/{version}-{fingerprint}.json`
//!
//! `cache_root` defaults to `$XDG_CACHE_HOME/async-doctor` (or `~/.cache/...`)
//! but callers (and tests) may inject an explicit root — never rely on mutating
//! process-wide env vars under parallel `cargo test`.
//!
//! Function keys are always the External qualname used at the call site
//! (e.g. `requests.api.get`, `libx.core.fetch`). Schema/tool versions plus a
//! package content fingerprint are part of the path so editable/patched
//! installs with the same version do not reuse a stale cache.

mod fingerprint;

pub use fingerprint::package_content_fingerprint;
pub(crate) use fingerprint::{fingerprint_from_record_bytes, mtime_stamp};

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Bump when the on-disk shape, keys, or extracted execution semantics change.
///
/// v4–v19: structural async-def gate, redis MRO keys, whole-scan content-hash
/// report cache, offload boundaries, report schemaVersion 2, class MRO /
/// `super()`, `followBudgetSkipped`, `cheap-path-op`, and named-fn guard fixes.
/// v20: named function bodies do not inherit definition-site execution guards.
/// v21: `CachedEdge` persists `is_bound_call`, `is_eager_offload_arg`,
/// `constructed_receiver`, `positional_externals`, and
/// `positional_constructed_receivers`; `follow_targets` preserved across partial
/// package updates; namespace & single-file package metadata indexed via `RECORD`.
/// v22: expanded async-twin `any_needs` symbol probing (`grpc.aio`, SQLAlchemy
/// `AsyncSession` / `AsyncEngine`) and first-party async anti-pattern edges
/// (`sync-for-async-iterable`, `eager-offload-call`, `unawaited-coroutine`,
/// `ephemeral-async-client`, `shared-async-session`).
/// v23: `CachedFunction` persists explicit `file` path (fixing zero-edge
/// `__init__.py` / `dist-packages` drift); cross-package callable quals resolve
/// after all waves materialize; sub-second mtime & unpackaged subdir fingerprints.
/// v24: `CachedEdge` persists multi-guard `param_guards` + `keyword_literals`
/// (conjunctive `ParamGuard`s, `else`/early-return inversion, keyword literals).
/// v25: deferred generator/context/callback edges and coroutine classification;
/// literal bindings include proven caller-parameter forwarding (`param:N`).
/// v26: concurrent Future guards use actual method identities and local
/// completion proofs that do not survive explicit receiver replacement.
/// v27: Future exception waits share typed completion guards; literal
/// nonpositive-timeout result/exception polls do not produce waiting edges.
pub const CACHE_SCHEMA_VERSION: u32 = 27;

/// Tool version embedded in the cache path (from this crate's Cargo.toml).
pub const TOOL_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Build identity for whole-scan report cache keys.
///
/// Combines compile-time crate/git build metadata (`ASYNC_DOCTOR_BUILD_ID` +
/// `TOOL_VERSION`) with an `O(1)` `stat` fingerprint of `current_exe()`
/// `(len, mtime_secs, mtime_nanos, dev, ino)` so rebuilds invalidate reuse
/// without streaming a ~28 MiB binary on every invocation.
pub fn build_id() -> &'static str {
    use std::sync::OnceLock;
    static ID: OnceLock<String> = OnceLock::new();
    ID.get_or_init(|| {
        std::env::current_exe()
            .ok()
            .and_then(|exe| compute_exe_build_id(&exe))
            .unwrap_or_else(|| env!("ASYNC_DOCTOR_BUILD_ID").to_string())
    })
    .as_str()
}

fn compute_exe_build_id(exe: &Path) -> Option<String> {
    let meta = fs::metadata(exe).ok()?;
    let len = meta.len();
    let (secs, nanos) = mtime_stamp(&meta);
    #[cfg(unix)]
    let (dev, ino) = {
        use std::os::unix::fs::MetadataExt;
        (meta.dev(), meta.ino())
    };
    #[cfg(not(unix))]
    let (dev, ino) = (0u64, 0u64);
    let stamp = format!(
        "build-v2:{TOOL_VERSION}:{}:{}:{len}:{secs}.{nanos}:{dev}:{ino}",
        env!("ASYNC_DOCTOR_BUILD_ID"),
        exe.display()
    );
    Some(format!("exe-{}", short_hash(stamp.as_bytes())))
}

fn fnv1a64_bytes(data: &[u8]) -> u64 {
    const PRIME: u64 = 0x100000001b3;
    let mut h0: u64 = 0xcbf29ce484222325 ^ (data.len() as u64);
    let mut h1: u64 = 0x84222325cbf29ce4;
    let mut h2: u64 = 0x4b95f51d84222325;
    let mut h3: u64 = 0x9e3779b97f4a7c15;

    let mut chunks = data.chunks_exact(32);
    for chunk in &mut chunks {
        let w0 = u64::from_le_bytes(chunk[0..8].try_into().unwrap());
        let w1 = u64::from_le_bytes(chunk[8..16].try_into().unwrap());
        let w2 = u64::from_le_bytes(chunk[16..24].try_into().unwrap());
        let w3 = u64::from_le_bytes(chunk[24..32].try_into().unwrap());
        h0 = (h0 ^ w0).wrapping_mul(PRIME);
        h1 = (h1 ^ w1).wrapping_mul(PRIME);
        h2 = (h2 ^ w2).wrapping_mul(PRIME);
        h3 = (h3 ^ w3).wrapping_mul(PRIME);
    }
    let mut h = h0
        .rotate_left(13)
        .wrapping_add(h1.rotate_left(27))
        .wrapping_add(h2.rotate_left(41))
        .wrapping_add(h3.rotate_left(53));
    for &b in chunks.remainder() {
        h ^= u64::from(b);
        h = h.wrapping_mul(PRIME);
    }
    h ^= h >> 33;
    h = h.wrapping_mul(0xff51afd7ed558ccd);
    h ^= h >> 33;
    h
}

/// One cached edge from a third-party function body (full summary).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CachedEdge {
    /// External / offload / third-party callee key (never `project:N`).
    pub callee_qualname: String,
    /// `external` | `offload` | `unresolved` | `param_invoke` | `third_party`
    pub callee_kind: String,
    pub offload_kind: Option<String>,
    pub param_invoke_index: Option<u32>,
    pub is_offload_wrapper: bool,
    /// `Call` | `WithEnter` | `WithExit` | `ForIter` | `PropertyGet` | `GeneratorConsume`
    pub kind: String,
    /// Stable site-packages-relative path (matches cold-extract `FunctionFact.file`).
    #[serde(default)]
    pub file: String,
    pub line: u32,
    pub text: String,
    pub conditions: Vec<String>,
    pub occurrence: u32,
    /// Positional bindings: `true` / `false` / `none` / `param:N` / `null`.
    pub positional_literals: Vec<Option<String>>,
    /// Keyword bindings (same encoding as positional bindings).
    #[serde(default)]
    pub keyword_literals: Vec<(String, Option<String>)>,
    /// Positional callable External/full quals when known.
    pub positional_callable_quals: Vec<Option<String>>,
    /// Positional external qualnames for offload / advice calls when known.
    #[serde(default)]
    pub positional_externals: Vec<Option<String>>,
    /// Positional constructed client roots when known.
    #[serde(default)]
    pub positional_constructed_receivers: Vec<Option<String>>,
    /// Keyword callable External/full quals when known (`name`, qual).
    #[serde(default)]
    pub keyword_callable_quals: Vec<(String, String)>,
    /// Conjunctive parameter guards `(param_index, inverted)`.
    #[serde(default)]
    pub param_guards: Vec<(u32, bool)>,
    /// Call was under `await` when recorded.
    #[serde(default)]
    pub is_awaited: bool,
    /// Resolved `async def` callee when known (`None` = unresolved / legacy cache).
    #[serde(default)]
    pub callee_is_coroutine: Option<bool>,
    /// Call evaluated eagerly inside an offload argument (`to_thread(f())`).
    #[serde(default)]
    pub is_eager_offload_arg: bool,
    /// Bound method / `@classmethod` call where parameter 0 (`self`/`cls`) is implicit.
    #[serde(default)]
    pub is_bound_call: bool,
    /// Constructed client root on the callee chain when known.
    #[serde(default)]
    pub constructed_receiver: Option<String>,
}

/// Per-function summary stored in the package cache.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CachedFunction {
    /// Canonical lookup key (= External qualname at the call site).
    pub key: String,
    pub qualname: String,
    pub module: String,
    /// Stable site-/dist-packages/editable-relative file path (`FunctionFact.file`).
    #[serde(default)]
    pub file: String,
    pub line: u32,
    pub is_async: bool,
    pub is_generator: bool,
    pub param_names: Vec<String>,
    pub invokes_params: Vec<u32>,
    pub is_offload_wrapper: bool,
    /// Decorator refs persisted as `module.qual` keys — restored as
    /// `DecoratorRef::External` on materialization.
    #[serde(default)]
    pub decorator_quals: Vec<String>,
    pub edges: Vec<CachedEdge>,
}

/// Enough to re-follow a cross-package External on a warm run without ty.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CachedFollowTarget {
    pub qualname: String,
    pub path: String,
    pub name_start: u32,
    pub package_name: Option<String>,
    pub package_version: Option<String>,
    pub package_fingerprint: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PackageCache {
    pub schema_version: u32,
    pub tool_version: String,
    pub package: String,
    pub version: String,
    /// Keyed by External qualname (sorted for stable files).
    pub functions: BTreeMap<String, CachedFunction>,
    /// Cross-package (and same-package) follow targets discovered while
    /// analyzing this package — warm runs re-inject these so later waves can
    /// extract callees that first-party code never mentioned directly.
    #[serde(default)]
    pub follow_targets: BTreeMap<String, CachedFollowTarget>,
}

impl PackageCache {
    pub fn new(package: &str, version: &str) -> Self {
        Self {
            schema_version: CACHE_SCHEMA_VERSION,
            tool_version: TOOL_VERSION.to_string(),
            package: package.to_string(),
            version: version.to_string(),
            functions: BTreeMap::new(),
            follow_targets: BTreeMap::new(),
        }
    }

    pub fn is_compatible(&self) -> bool {
        self.schema_version == CACHE_SCHEMA_VERSION && self.tool_version == TOOL_VERSION
    }
}

/// Resolve the default async-doctor user cache directory root.
pub fn default_cache_root() -> PathBuf {
    if let Ok(xdg) = std::env::var("XDG_CACHE_HOME")
        && !xdg.is_empty()
    {
        return PathBuf::from(xdg).join("async-doctor");
    }
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home).join(".cache").join("async-doctor");
    }
    std::env::temp_dir().join("async-doctor-cache")
}

/// Prefer an explicit root (tests / tooling); otherwise the user default.
pub fn effective_cache_root(explicit: Option<&Path>) -> PathBuf {
    explicit
        .map(Path::to_path_buf)
        .unwrap_or_else(default_cache_root)
}

pub(crate) fn sanitize_component(component: &str) -> String {
    component
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Opportunistically remove sibling `v{old}` schema directories older than
/// [`CACHE_SCHEMA_VERSION`] under `root`.
pub(crate) fn prune_stale_schema_dirs(root: &Path) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for ent in entries.flatten() {
        let fname = ent.file_name();
        let Some(name) = fname.to_str() else {
            continue;
        };
        let Some(ver_str) = name.strip_prefix('v') else {
            continue;
        };
        if let Ok(ver) = ver_str.parse::<u32>()
            && ver < CACHE_SCHEMA_VERSION
            && ent.file_type().is_ok_and(|ft| ft.is_dir())
        {
            let _ = fs::remove_dir_all(ent.path());
        }
    }
}

fn package_cache_path(root: &Path, package: &str, version: &str, fingerprint: &str) -> PathBuf {
    root.join(format!("v{CACHE_SCHEMA_VERSION}"))
        .join(sanitize_component(TOOL_VERSION))
        .join(sanitize_component(package))
        .join(format!(
            "{}-{}.json",
            sanitize_component(version),
            sanitize_component(fingerprint)
        ))
}

pub(crate) fn unique_tmp_path(path: &Path) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    path.with_extension(format!("json.tmp.{}.{seq}", std::process::id()))
}

pub(crate) fn short_hash(bytes: &[u8]) -> String {
    format!("{:016x}", fnv1a64_bytes(bytes))
}

/// Load a cached package summary when present and compatible.
pub fn load_package_cache(
    root: &Path,
    package: &str,
    version: &str,
    fingerprint: &str,
) -> Option<PackageCache> {
    let path = package_cache_path(root, package, version, fingerprint);
    let data = fs::read_to_string(path).ok()?;
    let cache: PackageCache = serde_json::from_str(&data).ok()?;
    if !cache.is_compatible() {
        return None;
    }
    if cache.package != package || cache.version != version {
        return None;
    }
    Some(cache)
}

/// Persist a package summary atomically (temp file + rename).
pub fn save_package_cache(
    root: &Path,
    cache: &PackageCache,
    fingerprint: &str,
) -> std::io::Result<()> {
    let path = package_cache_path(root, &cache.package, &cache.version, fingerprint);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let data = serde_json::to_string(cache)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    let tmp = unique_tmp_path(&path);
    if let Err(err) = fs::write(&tmp, &data).and_then(|()| fs::rename(&tmp, &path)) {
        let _ = fs::remove_file(&tmp);
        return Err(err);
    }
    Ok(())
}

/// True when `path` looks like a file under a `site-packages` tree.
#[allow(dead_code)]
pub fn is_site_packages_path(path: &Path) -> bool {
    path_str_contains_site_packages(&path.to_string_lossy())
}

pub fn path_str_contains_site_packages(s: &str) -> bool {
    path_str_is_third_party_python(s)
}

/// Split a `/`-normalized path into `(third_party_root, module_rel_path)` when it
/// resides under `site-packages`, `dist-packages`, or PDM `__pypackages__/<ver>/lib`.
pub(crate) fn third_party_root_split(s: &str) -> Option<(&str, &str)> {
    for marker in ["/site-packages/", "/dist-packages/"] {
        if let Some(idx) = s.rfind(marker) {
            let root_end = idx + marker.len() - 1;
            return Some((&s[..root_end], &s[idx + marker.len()..]));
        }
    }
    if let Some(idx) = s.rfind("/__pypackages__/") {
        let after = &s[idx + "/__pypackages__/".len()..];
        if let Some((ver, tail)) = after.split_once("/lib/")
            && !ver.is_empty()
            && !ver.contains('/')
        {
            let root_end = idx + "/__pypackages__/".len() + ver.len() + "/lib".len();
            return Some((&s[..root_end], tail));
        }
    }
    None
}

/// True when `path` is under a venv/user `site-packages`, system `dist-packages`,
/// or PDM `__pypackages__/<ver>/lib` tree (followed, never first-party entries).
pub fn path_str_is_third_party_python(s: &str) -> bool {
    if !s.contains('\\') {
        return third_party_root_split(s).is_some();
    }
    let s = s.replace('\\', "/");
    third_party_root_split(&s).is_some()
}

/// Module-relative path after the site-packages / dist-packages / PDM root, if any.
pub fn third_party_module_path(s: &str) -> Option<&str> {
    third_party_root_split(s).map(|(_, rest)| rest)
}

/// True when the path is a `.py` source file (not `.pyi` stub-only).
#[allow(dead_code)]
pub fn is_python_source(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()) == Some("py")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_roundtrip_atomic_and_keyed() {
        let dir =
            std::env::temp_dir().join(format!("async-doctor-cache-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let root = dir.join("async-doctor");

        let mut cache = PackageCache::new("demo", "1.0.0");
        cache.functions.insert(
            "demo.mod.fn".into(),
            CachedFunction {
                key: "demo.mod.fn".into(),
                qualname: "fn".into(),
                module: "demo.mod".into(),
                file: "site-packages/demo/mod.py".into(),
                line: 1,
                is_async: false,
                is_generator: false,
                param_names: vec![],
                invokes_params: vec![],
                is_offload_wrapper: false,
                decorator_quals: vec![],
                edges: vec![CachedEdge {
                    callee_qualname: "time.sleep".into(),
                    callee_kind: "external".into(),
                    offload_kind: None,
                    param_invoke_index: None,
                    is_offload_wrapper: false,
                    kind: "Call".into(),
                    file: "site-packages/demo/mod.py".into(),
                    line: 2,
                    text: "time.sleep(1)".into(),
                    conditions: vec!["unconditional".into(), "all-paths".into()],
                    occurrence: 0,
                    positional_literals: vec![],
                    keyword_literals: vec![],
                    positional_callable_quals: vec![],
                    positional_externals: vec![],
                    positional_constructed_receivers: vec![],
                    keyword_callable_quals: vec![],
                    param_guards: vec![],
                    is_awaited: false,
                    callee_is_coroutine: None,
                    is_eager_offload_arg: false,
                    is_bound_call: false,
                    constructed_receiver: None,
                }],
            },
        );
        save_package_cache(&root, &cache, "fpdeadbeef").unwrap();
        let loaded = load_package_cache(&root, "demo", "1.0.0", "fpdeadbeef").unwrap();
        assert_eq!(loaded.functions["demo.mod.fn"].key, "demo.mod.fn");
        assert_eq!(
            loaded.functions["demo.mod.fn"].file,
            "site-packages/demo/mod.py"
        );
        assert!(load_package_cache(&root, "demo", "1.0.0", "otherfp").is_none());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn stale_schema_cache_is_not_reused() {
        let dir =
            std::env::temp_dir().join(format!("async-doctor-cache-stale-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let root = dir.join("async-doctor");

        let mut cache = PackageCache::new("demo", "1.0.0");
        assert_eq!(cache.schema_version, CACHE_SCHEMA_VERSION);
        assert!(cache.is_compatible());
        for stale in [3u32, 10, 21, 22, 23] {
            cache.schema_version = stale;
            assert!(!cache.is_compatible());
        }

        cache.schema_version = CACHE_SCHEMA_VERSION;
        save_package_cache(&root, &cache, "fpstale").unwrap();
        let path = package_cache_path(&root, "demo", "1.0.0", "fpstale");
        for stale in [3u32, 10, 21, 22, 23] {
            let mut spoofed: serde_json::Value =
                serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
            spoofed["schema_version"] = serde_json::json!(stale);
            fs::write(&path, serde_json::to_string(&spoofed).unwrap()).unwrap();
            assert!(load_package_cache(&root, "demo", "1.0.0", "fpstale").is_none());
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn stat_build_id_tracks_size_and_subsec_mtime() {
        use std::time::{Duration, UNIX_EPOCH};
        let tmp = tempfile::tempdir().unwrap();
        let bin = tmp.path().join("fake-bin");
        fs::write(&bin, b"binary-v1").unwrap();
        let f = fs::OpenOptions::new().write(true).open(&bin).unwrap();
        f.set_modified(UNIX_EPOCH + Duration::new(1_700_000_000, 111_000_000))
            .unwrap();
        let id1 = compute_exe_build_id(&bin).unwrap();
        f.set_modified(UNIX_EPOCH + Duration::new(1_700_000_000, 222_000_000))
            .unwrap();
        let id2 = compute_exe_build_id(&bin).unwrap();
        assert_ne!(id1, id2, "sub-second mtime change must update build_id");
        fs::write(&bin, b"binary-v2-longer").unwrap();
        let id3 = compute_exe_build_id(&bin).unwrap();
        assert_ne!(id2, id3);
        assert!(build_id().starts_with("exe-"));
    }
}
