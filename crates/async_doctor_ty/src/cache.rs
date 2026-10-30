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
pub const CACHE_SCHEMA_VERSION: u32 = 5;

/// Tool version embedded in the cache path (from this crate's Cargo.toml).
pub const TOOL_VERSION: &str = env!("CARGO_PKG_VERSION");

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
    /// Positional callable External/full quals when known.
    pub positional_callable_quals: Vec<Option<String>>,
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

fn sanitize(component: &str) -> String {
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
        .join(sanitize(TOOL_VERSION))
        .join(sanitize(package))
        .join(format!(
            "{}-{}.json",
            sanitize(version),
            sanitize(fingerprint)
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

    // No dist-info: shallow .py fingerprint only.
    let base = if dist_info_or_pkg_dir.is_file() {
        dist_info_or_pkg_dir
            .parent()
            .unwrap_or(dist_info_or_pkg_dir)
    } else {
        dist_info_or_pkg_dir
    };
    let mut entries: Vec<(String, u64, u64)> = Vec::new();
    if let Ok(walker) = fs::read_dir(base) {
        for ent in walker.flatten() {
            let p = ent.path();
            if p.extension().and_then(|e| e.to_str()) != Some("py") {
                continue;
            }
            if let Ok(meta) = ent.metadata() {
                let len = meta.len();
                let mtime = meta
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                entries.push((
                    p.file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("")
                        .to_string(),
                    len,
                    mtime,
                ));
            }
        }
    }
    entries.sort();
    short_hash(format!("PY:{entries:?}").as_bytes())
}

fn short_hash(bytes: &[u8]) -> String {
    // Cheap stable FNV-1a 64-bit → 16 hex chars (no extra crate).
    let mut hash: u64 = 0xcbf29ce484222325;
    for b in bytes {
        hash ^= u64::from(*b);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
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
    let tmp = path.with_extension(format!("json.tmp.{}", std::process::id()));
    fs::write(&tmp, &data)?;
    fs::rename(&tmp, &path)?;
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
                    positional_callable_quals: vec![],
                    keyword_callable_quals: vec![],
                    param_guard_index: None,
                    param_guard_inverted: None,
                    is_awaited: false,
                    callee_is_coroutine: None,
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
        // v4 added `callee_is_coroutine`; v5 changed redis-subclass External
        // keys. Older schema_version documents (or v3/v4 directories) must miss.
        let dir =
            std::env::temp_dir().join(format!("async-doctor-cache-stale-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let root = dir.join("async-doctor");

        let mut cache = PackageCache::new("demo", "1.0.0");
        assert_eq!(cache.schema_version, CACHE_SCHEMA_VERSION);
        assert!(cache.is_compatible());
        for stale in [3u32, 4u32] {
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
        for stale in [3u32, 4u32] {
            let mut spoofed: serde_json::Value =
                serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
            spoofed["schema_version"] = serde_json::json!(stale);
            fs::write(&path, serde_json::to_string(&spoofed).unwrap()).unwrap();
            assert!(
                load_package_cache(&root, "demo", "1.0.0", "fpstale").is_none(),
                "schema {stale} in the v{CACHE_SCHEMA_VERSION} path must not load"
            );
        }

        // Legacy directory layouts (v3/, v4/) must also miss.
        for stale in [3u32, 4u32] {
            let stale_dir = root
                .join(format!("v{stale}"))
                .join(sanitize(TOOL_VERSION))
                .join(sanitize("demo"));
            fs::create_dir_all(&stale_dir).unwrap();
            let stale_path = stale_dir.join(format!(
                "{}-{}.json",
                sanitize("1.0.0"),
                sanitize("fpstale")
            ));
            let mut stale_doc = PackageCache::new("demo", "1.0.0");
            stale_doc.schema_version = stale;
            fs::write(&stale_path, serde_json::to_string(&stale_doc).unwrap()).unwrap();
        }
        assert!(
            load_package_cache(&root, "demo", "1.0.0", "fpstale").is_none(),
            "files under v3/ or v4/ must not be reused by schema v{CACHE_SCHEMA_VERSION}"
        );

        let _ = fs::remove_dir_all(&dir);
    }
}
