//! On-disk cache for third-party (site-packages) analysis results.
//!
//! Layout:
//! `$XDG_CACHE_HOME/async-doctor/v{schema}/{tool}/{package}/{version}.json`
//! (falls back to `~/.cache/async-doctor/...`).
//!
//! Function keys are always the External qualname used at the call site
//! (e.g. `requests.api.get`, `libx.core.fetch`). Schema/tool versions are
//! part of the path so stale caches never load.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Bump when the on-disk JSON shape changes incompatibly.
pub const CACHE_SCHEMA_VERSION: u32 = 2;

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
    pub param_guard_index: Option<u32>,
    pub param_guard_inverted: Option<bool>,
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PackageCache {
    pub schema_version: u32,
    pub tool_version: String,
    pub package: String,
    pub version: String,
    /// Keyed by External qualname (sorted for stable files).
    pub functions: BTreeMap<String, CachedFunction>,
}

impl PackageCache {
    pub fn new(package: &str, version: &str) -> Self {
        Self {
            schema_version: CACHE_SCHEMA_VERSION,
            tool_version: TOOL_VERSION.to_string(),
            package: package.to_string(),
            version: version.to_string(),
            functions: BTreeMap::new(),
        }
    }

    pub fn is_compatible(&self) -> bool {
        self.schema_version == CACHE_SCHEMA_VERSION && self.tool_version == TOOL_VERSION
    }
}

/// Resolve the async-doctor user cache directory root (before versioned subdirs).
pub fn cache_root() -> PathBuf {
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

fn package_cache_path(package: &str, version: &str) -> PathBuf {
    cache_root()
        .join(format!("v{CACHE_SCHEMA_VERSION}"))
        .join(sanitize(TOOL_VERSION))
        .join(sanitize(package))
        .join(format!("{}.json", sanitize(version)))
}

/// Load a cached package summary when present and compatible.
pub fn load_package_cache(package: &str, version: &str) -> Option<PackageCache> {
    let path = package_cache_path(package, version);
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
pub fn save_package_cache(cache: &PackageCache) -> std::io::Result<()> {
    let path = package_cache_path(&cache.package, &cache.version);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let data = serde_json::to_string(cache)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, &data)?;
    // rename is atomic on the same filesystem.
    fs::rename(&tmp, &path)?;
    Ok(())
}

/// True when `path` looks like a file under a `site-packages` tree.
#[allow(dead_code)]
pub fn is_site_packages_path(path: &Path) -> bool {
    path_str_contains_site_packages(&path.to_string_lossy())
}

pub fn path_str_contains_site_packages(s: &str) -> bool {
    let s = s.replace('\\', "/");
    s.contains("/site-packages/")
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
        // SAFETY: isolated test dir via XDG_CACHE_HOME.
        unsafe { std::env::set_var("XDG_CACHE_HOME", &dir) };

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
                    param_guard_index: None,
                    param_guard_inverted: None,
                }],
            },
        );
        save_package_cache(&cache).unwrap();
        let loaded = load_package_cache("demo", "1.0.0").unwrap();
        assert_eq!(
            loaded.functions.get("demo.mod.fn").unwrap().key,
            "demo.mod.fn"
        );
        assert!(loaded.functions.contains_key("demo.mod.fn"));
        // Wrong key format must miss.
        assert!(!loaded.functions.contains_key("demo.mod:fn"));

        let _ = fs::remove_dir_all(&dir);
        unsafe { std::env::remove_var("XDG_CACHE_HOME") };
    }
}
