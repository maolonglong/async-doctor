//! Site-packages helpers: dist-info metadata, follow targets, and package cache I/O.

mod batch;

pub use batch::PackageCacheBatch;
pub(crate) use batch::materialize_cached_functions;

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use ruff_db::system::SystemPathBuf;

use crate::cache::{package_content_fingerprint, path_str_is_third_party_python};

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

/// Per-scan memo for package meta lookups.
///
/// `meta_for_path` used to re-`read_dir` + fingerprint on every call site; one
/// large venv made that dominate wall/sys time. Owned by the scan context —
/// never process-global — so concurrent scans cannot share or clobber state.
#[derive(Debug, Default)]
pub struct PackageMetaMemo {
    map: HashMap<PackageMetaKey, PackageMetaValue>,
}

impl PackageMetaMemo {
    pub fn new() -> Self {
        Self::default()
    }

    /// Locate neighboring `*.dist-info` for a file under site-/dist-packages.
    /// Returns `(name, version, content_fingerprint)`.
    pub fn meta_for_path(&mut self, py_path: &Path) -> Option<(String, String, String)> {
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
        if let Some(cached) = self.map.get(&key) {
            return cached.clone();
        }

        let computed = package_meta_uncached(&root, &top);
        self.map.insert(key, computed.clone());
        computed
    }

    /// Number of memoized package keys (test seam).
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.map.len()
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn package_meta_memo_is_per_instance_not_global() {
        let dir = tempfile::tempdir().unwrap();
        let site = dir.path().join("lib/python3.12/site-packages");
        let pkg = site.join("demo");
        let dist = site.join("demo-1.0.0.dist-info");
        fs::create_dir_all(&pkg).unwrap();
        fs::create_dir_all(&dist).unwrap();
        fs::write(pkg.join("__init__.py"), "").unwrap();
        fs::write(dist.join("METADATA"), "Name: demo\nVersion: 1.0.0\n").unwrap();
        fs::write(dist.join("RECORD"), "demo/__init__.py,,\n").unwrap();

        let py = pkg.join("__init__.py");
        let mut a = PackageMetaMemo::new();
        let mut b = PackageMetaMemo::new();
        let ma = a.meta_for_path(&py).unwrap();
        let mb = b.meta_for_path(&py).unwrap();
        assert_eq!(ma.0, "demo");
        assert_eq!(mb.0, "demo");
        assert_eq!(a.len(), 1);
        assert_eq!(b.len(), 1);
        // Filling `a` further must not grow `b`.
        let _ = a.meta_for_path(&py);
        assert_eq!(a.len(), 1);
        assert_eq!(b.len(), 1);
    }
}
