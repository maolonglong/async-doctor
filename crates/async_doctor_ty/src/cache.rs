//! On-disk cache for third-party (site-packages) analysis results.
//!
//! Layout: `$XDG_CACHE_HOME/async-doctor/{package}/{version}.json`
//! (falls back to `~/.cache/async-doctor`).

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// One cached edge from a third-party function body.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedEdge {
    /// External or same-package callee qualified name.
    pub callee_qualname: String,
    /// `external` | `offload` | `unresolved`
    pub callee_kind: String,
    pub offload_kind: Option<String>,
    pub is_offload_wrapper: bool,
    pub line: u32,
    pub text: String,
}

/// Per-function summary stored in the package cache.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedFunction {
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackageCache {
    pub package: String,
    pub version: String,
    pub functions: HashMap<String, CachedFunction>,
}

/// Resolve the async-doctor user cache directory.
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
        .join(sanitize(package))
        .join(format!("{}.json", sanitize(version)))
}

/// Load a cached package summary when present.
pub fn load_package_cache(package: &str, version: &str) -> Option<PackageCache> {
    let path = package_cache_path(package, version);
    let data = fs::read_to_string(path).ok()?;
    serde_json::from_str(&data).ok()
}

/// Persist a package summary for reuse on the next run.
pub fn save_package_cache(cache: &PackageCache) -> std::io::Result<()> {
    let path = package_cache_path(&cache.package, &cache.version);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let data = serde_json::to_string_pretty(cache)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    fs::write(path, data)
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
