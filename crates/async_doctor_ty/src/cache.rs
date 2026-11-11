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

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Bump when the on-disk JSON shape / key layout changes incompatibly.
///
/// v4: edges carry `callee_is_coroutine` (structural async-def gate).
/// v5: redis subclass Externals key off receiver MRO (asyncio base ⇒
/// `redis.asyncio.Redis.*`). A v4 cache written before that fix still stores
/// sync `redis.Redis.*` for asyncio subclasses and must not be reused.
/// v6: briefly emitted unresolved Name/Attribute call edges (reverted in v7).
/// v7: restore call-edge silence for plain Name/Attribute; failed imports are
/// recorded separately; search paths append after src_roots (skip root dupes).
/// v8: outside editables are follow-only (not first-party entries); unresolved
/// imports bounded to first-party head misses + one third-party aggregate.
/// v9: relative imports that exist on disk (esp. when scan root is the package)
/// are not recorded as unresolved-import.
/// v10: whole-scan report cache under `scan/{fingerprint}.json` (warm fast
/// path when first-party + venv inputs are unchanged).
/// v11: whole-scan fingerprint uses first-party **content hashes** (not
/// whole-second mtime+size), plus venv dist-info fingerprints and tool/schema.
/// v12: asgiref.sync.sync_to_async offload boundary; advice-tier twin
/// resolution (`resolved_symbols`); offload positional externals /
/// coroutine-target facts for nested-event-loop / offload-coroutine /
/// offload-has-async-twin.
/// v13: `no_running_loop_guard` condition tag; advice ignores offload-site
/// try/if/loop gates; lambda body edges graded relative to the lambda;
/// httpx module + Azure OpenAI twin matchers.
/// v14: report schemaVersion 2 — `environment`, `locationLessCandidates`,
/// always-emitted top-level arrays.
/// v15: edge occurrence for externals counted by canonical sink key (family
/// aliases like Session.client → boto3.client) so finding ids cannot collide.
/// v16: project class constructor / MRO / `super()` resolution; `Path.mkdir` +
/// `importlib.resources` Traversable sinks; `open`+`read`/`write` sink-site
/// attribution; log-scaled health score.
/// v17: offload decorators (`sync_to_async` / `database_sync_to_async` /
/// project wrappers) cut call edges; `functools.cached_property` getters
/// followed; non-IO methods (`httpx.Client.build_request`,
/// `requests.Session.mount|get_adapter`) leave the sink families.
/// v18: `environment.followBudgetSkipped` counts budget-truncated
/// third-party follow candidates.
/// v19: edges record keyword args passed a literal `True` (`cheap-path-op`).
/// v20: named function bodies do not inherit definition-site execution guards.
/// v21: `CachedEdge` persists `is_bound_call`, `is_eager_offload_arg`,
/// `constructed_receiver`, `positional_externals`, and
/// `positional_constructed_receivers`; `follow_targets` preserved across partial
/// package updates; namespace & single-file package metadata indexed via `RECORD`.
pub const CACHE_SCHEMA_VERSION: u32 = 21;

/// Tool version embedded in the cache path (from this crate's Cargo.toml).
pub const TOOL_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Build identity for whole-scan report cache keys.
///
/// Hashed from the running executable so edits in any linked crate (or the
/// workspace lockfile) invalidate reuse after rebuild — without relying on
/// `build.rs` rerun-if-changed coverage of every path.
pub fn build_id() -> &'static str {
    use std::sync::OnceLock;
    static ID: OnceLock<String> = OnceLock::new();
    ID.get_or_init(|| {
        if let Ok(exe) = std::env::current_exe()
            && let Ok(h) = hash_file_streaming(&exe)
        {
            return format!("exe-{h:x}");
        }
        // Fallback when the exe cannot be read (unusual): compile-time stamp.
        env!("ASYNC_DOCTOR_BUILD_ID").to_string()
    })
    .as_str()
}

fn hash_file_streaming(path: &Path) -> std::io::Result<u64> {
    let f = fs::File::open(path)?;
    hash_reader_streaming(f)
}

fn hash_reader_streaming(mut reader: impl std::io::Read) -> std::io::Result<u64> {
    const PRIME: u64 = 0x100000001b3;
    let mut h0: u64 = 0xcbf29ce484222325;
    let mut h1: u64 = 0x84222325cbf29ce4;
    let mut h2: u64 = 0x4b95f51d84222325;
    let mut h3: u64 = 0x9e3779b97f4a7c15;
    let mut total_len: u64 = 0;
    let mut tail = [0u8; 32];
    let mut tail_len = 0usize;
    let mut buf = [0u8; 64 * 1024];

    let mix_block = |chunk: &[u8], h0: &mut u64, h1: &mut u64, h2: &mut u64, h3: &mut u64| {
        let w0 = u64::from_le_bytes(chunk[0..8].try_into().unwrap());
        let w1 = u64::from_le_bytes(chunk[8..16].try_into().unwrap());
        let w2 = u64::from_le_bytes(chunk[16..24].try_into().unwrap());
        let w3 = u64::from_le_bytes(chunk[24..32].try_into().unwrap());
        *h0 = (*h0 ^ w0).wrapping_mul(PRIME);
        *h1 = (*h1 ^ w1).wrapping_mul(PRIME);
        *h2 = (*h2 ^ w2).wrapping_mul(PRIME);
        *h3 = (*h3 ^ w3).wrapping_mul(PRIME);
    };

    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        total_len += n as u64;
        let mut slice = &buf[..n];
        if tail_len > 0 {
            let need = (32 - tail_len).min(slice.len());
            tail[tail_len..tail_len + need].copy_from_slice(&slice[..need]);
            tail_len += need;
            slice = &slice[need..];
            if tail_len == 32 {
                mix_block(&tail, &mut h0, &mut h1, &mut h2, &mut h3);
                tail_len = 0;
            }
        }
        let mut chunks = slice.chunks_exact(32);
        for chunk in &mut chunks {
            mix_block(chunk, &mut h0, &mut h1, &mut h2, &mut h3);
        }
        let rem = chunks.remainder();
        if !rem.is_empty() {
            tail[..rem.len()].copy_from_slice(rem);
            tail_len = rem.len();
        }
    }

    let mut h = (h0 ^ total_len)
        .rotate_left(13)
        .wrapping_add(h1.rotate_left(27))
        .wrapping_add(h2.rotate_left(41))
        .wrapping_add(h3.rotate_left(53));
    for &b in &tail[..tail_len] {
        h ^= u64::from(b);
        h = h.wrapping_mul(PRIME);
    }
    h ^= h >> 33;
    h = h.wrapping_mul(0xff51afd7ed558ccd);
    h ^= h >> 33;
    Ok(h)
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
    /// Positional literals: `true` / `false` / `none` / omitted (`null`).
    pub positional_literals: Vec<Option<String>>,
    /// Keyword argument names passed a literal `True`.
    #[serde(default)]
    pub keyword_true: Vec<String>,
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
    pub param_guard_index: Option<u32>,
    pub param_guard_inverted: Option<bool>,
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

/// Fingerprint package contents so editable/patched installs invalidate cache.
///
/// Cheap path (preferred):
/// 1. Hash `RECORD` bytes when present (covers wheel installs).
/// 2. Else hash `METADATA` len+mtime plus the dist-info directory mtime.
/// 3. Else (no dist-info): one-level `(name,len,mtime)` of `.py` files in the
///    package dir — never a recursive walk (that dominated sys time on large
///    venvs when called per call-site before memoization).
pub fn package_content_fingerprint(dist_info_or_pkg_dir: &Path) -> String {
    let dist_info = if dist_info_or_pkg_dir
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.ends_with(".dist-info"))
    {
        dist_info_or_pkg_dir.to_path_buf()
    } else {
        PathBuf::new()
    };

    if dist_info.is_dir() {
        let record = dist_info.join("RECORD");
        if record.is_file()
            && let Ok(bytes) = fs::read(&record)
        {
            // Mix in dist-info mtime so a reinstall that rewrites RECORD with
            // identical bytes still invalidates (rare) and editable installs
            // that refresh METADATA bump the fingerprint without a recursive
            // `.py` walk.
            let mut blob = Vec::with_capacity(bytes.len() + 32);
            blob.extend_from_slice(b"RECORD:");
            blob.extend_from_slice(&bytes);
            if let Ok(meta) = fs::metadata(&dist_info) {
                let mtime = meta
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                blob.extend_from_slice(format!("|DI:{mtime}").as_bytes());
            }
            return short_hash(&blob);
        }
        let meta_path = dist_info.join("METADATA");
        if let Ok(meta) = fs::metadata(&meta_path) {
            let len = meta.len();
            let mtime = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0);
            return short_hash(format!("META:{len}:{mtime}").as_bytes());
        }
    }

    // No dist-info: single-file module or shallow/bounded .py fingerprint.
    if dist_info_or_pkg_dir.is_file() {
        if let Ok(meta) = fs::metadata(dist_info_or_pkg_dir) {
            let len = meta.len();
            let mtime = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let name = dist_info_or_pkg_dir
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("");
            return short_hash(format!("PYFILE:{name}:{len}:{mtime}").as_bytes());
        }
        return short_hash(b"PYFILE:missing");
    }

    let mut entries: Vec<(String, u64, u64)> = Vec::new();
    collect_py_entries_bounded(dist_info_or_pkg_dir, "", 0, 2, &mut entries);
    entries.sort();
    short_hash(format!("PY:{entries:?}").as_bytes())
}

fn collect_py_entries_bounded(
    dir: &Path,
    prefix: &str,
    depth: usize,
    max_depth: usize,
    entries: &mut Vec<(String, u64, u64)>,
) {
    let Ok(walker) = fs::read_dir(dir) else {
        return;
    };
    let mut subdirs = Vec::new();
    for ent in walker.flatten() {
        let p = ent.path();
        let fname = ent.file_name();
        let Some(name) = fname.to_str() else {
            continue;
        };
        if p.extension().and_then(|e| e.to_str()) == Some("py") {
            if let Ok(meta) = ent.metadata() {
                let len = meta.len();
                let mtime = meta
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                let rel = if prefix.is_empty() {
                    name.to_string()
                } else {
                    format!("{prefix}/{name}")
                };
                entries.push((rel, len, mtime));
            }
        } else if depth < max_depth
            && name != "__pycache__"
            && ent.file_type().is_ok_and(|ft| ft.is_dir())
        {
            subdirs.push((p, name.to_string()));
        }
    }
    // Only descend into subdirectories when no top-level `.py` files exist (e.g. PEP 420 namespace root).
    if entries.is_empty() {
        subdirs.sort_by(|a, b| a.1.cmp(&b.1));
        for (sub_path, sub_name) in subdirs {
            let next_prefix = if prefix.is_empty() {
                sub_name
            } else {
                format!("{prefix}/{sub_name}")
            };
            collect_py_entries_bounded(&sub_path, &next_prefix, depth + 1, max_depth, entries);
        }
    }
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
///
/// Temp path includes pid so concurrent scanners sharing a cache dir cannot
/// clobber each other's in-flight writes.
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

/// True when `path` is under a venv/user `site-packages` or system `dist-packages`
/// tree. Both are treated as third-party (followed, never entries).
pub fn path_str_is_third_party_python(s: &str) -> bool {
    if !s.contains('\\') {
        return s.contains("/site-packages/") || s.contains("/dist-packages/");
    }
    let s = s.replace('\\', "/");
    s.contains("/site-packages/") || s.contains("/dist-packages/")
}

/// Module-relative path after the site-packages / dist-packages root, if any.
/// `s` should use `/` separators (callers typically normalize first).
pub fn third_party_module_path(s: &str) -> Option<&str> {
    if let Some(idx) = s.rfind("/site-packages/") {
        return Some(&s[idx + "/site-packages/".len()..]);
    }
    if let Some(idx) = s.rfind("/dist-packages/") {
        return Some(&s[idx + "/dist-packages/".len()..]);
    }
    None
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
                    keyword_true: vec![],
                    positional_callable_quals: vec![],
                    positional_externals: vec![],
                    positional_constructed_receivers: vec![],
                    keyword_callable_quals: vec![],
                    param_guard_index: None,
                    param_guard_inverted: None,
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
        assert_eq!(
            loaded.functions.get("demo.mod.fn").unwrap().key,
            "demo.mod.fn"
        );
        assert!(loaded.functions.contains_key("demo.mod.fn"));
        assert!(!loaded.functions.contains_key("demo.mod:fn"));
        // Different fingerprint must miss.
        assert!(load_package_cache(&root, "demo", "1.0.0", "otherfp").is_none());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn stale_schema_cache_is_not_reused() {
        // v4–v12 prior schemas must not load under the current version.
        let dir =
            std::env::temp_dir().join(format!("async-doctor-cache-stale-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let root = dir.join("async-doctor");

        let mut cache = PackageCache::new("demo", "1.0.0");
        assert_eq!(cache.schema_version, CACHE_SCHEMA_VERSION);
        assert!(cache.is_compatible());
        for stale in [3u32, 4u32, 5u32, 6u32, 7u32, 8u32, 9u32, 10u32, 11u32] {
            cache.schema_version = stale;
            assert!(
                !cache.is_compatible(),
                "schema {stale} must be incompatible with v{CACHE_SCHEMA_VERSION}"
            );
        }

        // Spoof stale schema_version into the *current* package path.
        cache.schema_version = CACHE_SCHEMA_VERSION;
        save_package_cache(&root, &cache, "fpstale").unwrap();
        let path = package_cache_path(&root, "demo", "1.0.0", "fpstale");
        for stale in [3u32, 4u32, 5u32, 6u32, 7u32, 8u32, 9u32, 10u32, 11u32] {
            let mut spoofed: serde_json::Value =
                serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
            spoofed["schema_version"] = serde_json::json!(stale);
            fs::write(&path, serde_json::to_string(&spoofed).unwrap()).unwrap();
            assert!(
                load_package_cache(&root, "demo", "1.0.0", "fpstale").is_none(),
                "schema {stale} in the v{CACHE_SCHEMA_VERSION} path must not load"
            );
        }

        // Legacy directory layouts must also miss.
        for stale in [3u32, 4u32, 5u32, 6u32, 7u32, 8u32, 9u32, 10u32, 11u32] {
            let stale_dir = root
                .join(format!("v{stale}"))
                .join(sanitize_component(TOOL_VERSION))
                .join(sanitize_component("demo"));
            fs::create_dir_all(&stale_dir).unwrap();
            let stale_path = stale_dir.join(format!(
                "{}-{}.json",
                sanitize_component("1.0.0"),
                sanitize_component("fpstale")
            ));
            let mut stale_doc = PackageCache::new("demo", "1.0.0");
            stale_doc.schema_version = stale;
            fs::write(&stale_path, serde_json::to_string(&stale_doc).unwrap()).unwrap();
        }
        assert!(
            load_package_cache(&root, "demo", "1.0.0", "fpstale").is_none(),
            "legacy schema dirs must not be reused by schema v{CACHE_SCHEMA_VERSION}"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn streaming_hash_is_chunk_boundary_independent() {
        struct ChunkedReader<'a> {
            data: &'a [u8],
            step: usize,
        }
        impl std::io::Read for ChunkedReader<'_> {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                if self.data.is_empty() {
                    return Ok(0);
                }
                let n = self.step.min(buf.len()).min(self.data.len());
                buf[..n].copy_from_slice(&self.data[..n]);
                self.data = &self.data[n..];
                Ok(n)
            }
        }
        let payload: Vec<u8> = (0..10_007u32)
            .map(|i| (i.wrapping_mul(31) & 0xff) as u8)
            .collect();
        let whole = hash_reader_streaming(std::io::Cursor::new(&payload)).unwrap();
        for step in [1usize, 7, 31, 32, 33, 64, 1023, 4096] {
            let chunked = hash_reader_streaming(ChunkedReader {
                data: &payload,
                step,
            })
            .unwrap();
            assert_eq!(whole, chunked, "mismatch for read step={step}");
        }
    }
}
