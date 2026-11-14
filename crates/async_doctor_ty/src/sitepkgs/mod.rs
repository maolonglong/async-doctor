//! Site-packages helpers: dist-info metadata, follow targets, and package cache I/O.

mod batch;

pub use batch::PackageCacheBatch;
#[cfg(test)]
pub(crate) use batch::materialize_cached_functions;
pub(crate) use batch::{DeferredCallablePatch, materialize_cached_functions_with_deferred};

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use ruff_db::system::SystemPathBuf;

use crate::cache::{
    fingerprint_from_record_bytes, package_content_fingerprint, path_str_is_third_party_python,
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

type PackageMetaTriple = (String, String, String);

#[derive(Debug, Clone)]
struct SiteRootIndex {
    dists: Vec<PackageMetaTriple>,
    /// Exact relative file path (e.g. `six.py`, `google/cloud/storage/client.py`) -> dist index.
    by_file: HashMap<String, usize>,
    /// Relative directory prefix -> `Some(dist_idx)` when unique, `None` when shared (PEP 420 namespace).
    by_dir: HashMap<String, Option<usize>>,
    /// Normalized distribution name -> dist index (fallback when RECORD omits files).
    by_norm_dist: HashMap<String, usize>,
}

impl SiteRootIndex {
    fn build(root: &Path) -> Self {
        let mut index = Self {
            dists: Vec::new(),
            by_file: HashMap::new(),
            by_dir: HashMap::new(),
            by_norm_dist: HashMap::new(),
        };
        let Ok(entries) = fs::read_dir(root) else {
            return index;
        };
        let mut dist_dirs: Vec<PathBuf> = entries
            .flatten()
            .filter_map(|e| {
                let fname = e.file_name();
                fname
                    .to_str()
                    .is_some_and(|s| s.ends_with(".dist-info"))
                    .then(|| e.path())
            })
            .collect();
        dist_dirs.sort();

        for dist_info in dist_dirs {
            let Ok(meta_text) = fs::read_to_string(dist_info.join("METADATA")) else {
                continue;
            };
            let Some((name, ver)) = parse_dist_info_metadata(&meta_text) else {
                continue;
            };
            let record_bytes = fs::read(dist_info.join("RECORD")).ok();
            let fp = if let Some(bytes) = &record_bytes {
                fingerprint_from_record_bytes(&dist_info, bytes)
            } else {
                package_content_fingerprint(&dist_info)
            };
            let dist_idx = index.dists.len();
            index
                .by_norm_dist
                .entry(normalize_dist_name(&name))
                .or_insert(dist_idx);
            if let Some(fname) = dist_info.file_name().and_then(|n| n.to_str()) {
                let stem = fname.trim_end_matches(".dist-info");
                let dist_stem = stem.split('-').next().unwrap_or(stem);
                index
                    .by_norm_dist
                    .entry(normalize_dist_name(dist_stem))
                    .or_insert(dist_idx);
            }
            index.dists.push((name, ver, fp));

            if let Some(bytes) = &record_bytes
                && let Ok(record_text) = std::str::from_utf8(bytes)
            {
                for raw_line in record_text.lines() {
                    let rel = raw_line.split(',').next().unwrap_or("").trim();
                    let rel = rel.trim_matches('"');
                    if rel.is_empty()
                        || rel.starts_with("..")
                        || rel.contains(".dist-info/")
                        || rel.ends_with(".pyc")
                    {
                        continue;
                    }
                    let rel_norm = if rel.contains('\\') {
                        rel.replace('\\', "/")
                    } else {
                        rel.to_string()
                    };
                    if rel_norm.ends_with(".py") {
                        index.by_file.entry(rel_norm.clone()).or_insert(dist_idx);
                    }
                    let mut cur = rel_norm.as_str();
                    while let Some((parent, _)) = cur.rsplit_once('/') {
                        if parent.is_empty() {
                            break;
                        }
                        index.record_dir(parent, dist_idx);
                        cur = parent;
                    }
                }
            }

            if let Ok(top_text) = fs::read_to_string(dist_info.join("top_level.txt")) {
                for line in top_text.lines() {
                    let top = line.trim().trim_matches('/');
                    if top.is_empty() {
                        continue;
                    }
                    index.record_dir(top, dist_idx);
                    index.by_file.entry(format!("{top}.py")).or_insert(dist_idx);
                }
            }
        }
        index
    }

    fn record_dir(&mut self, dir: &str, dist_idx: usize) {
        match self.by_dir.get_mut(dir) {
            None => {
                self.by_dir.insert(dir.to_string(), Some(dist_idx));
            }
            Some(slot @ Some(_)) => {
                if *slot != Some(dist_idx) {
                    *slot = None;
                }
            }
            Some(None) => {}
        }
    }

    fn lookup(&self, rest: &str) -> Option<&PackageMetaTriple> {
        if let Some(&idx) = self.by_file.get(rest) {
            return Some(&self.dists[idx]);
        }
        let mut cur = rest;
        while let Some((parent, _)) = cur.rsplit_once('/') {
            if parent.is_empty() {
                break;
            }
            match self.by_dir.get(parent) {
                Some(Some(idx)) => return Some(&self.dists[*idx]),
                Some(None) => {
                    // Ambiguous namespace directory (e.g. `google`): do not match by top-level stem.
                    return None;
                }
                None => {}
            }
            cur = parent;
        }
        let stem = match rest.split_once('/') {
            Some((top, _)) => top,
            None => rest.strip_suffix(".py").unwrap_or(rest),
        };
        if matches!(self.by_dir.get(stem), Some(None)) {
            return None;
        }
        let idx = *self.by_norm_dist.get(&normalize_dist_name(stem))?;
        Some(&self.dists[idx])
    }
}

/// Per-scan memo and pre-built site-packages index for package meta lookups.
///
/// Pre-indexing active `site-packages` directories once per scan avoids
/// repeated `read_dir` and `RECORD` reads across Pass-2 worker threads while
/// accurately resolving PEP 420 namespace packages (`google.*`), renamed wheels
/// (`PyYAML` -> `yaml`), and single-file modules (`six.py`). Owned per scan —
/// never process-global — so concurrent scans cannot clobber state.
#[derive(Debug, Default, Clone)]
pub struct PackageMetaMemo {
    preindexed: Arc<HashMap<String, SiteRootIndex>>,
    dynamic_roots: HashMap<String, SiteRootIndex>,
    fallback_map: HashMap<(String, String), Option<PackageMetaTriple>>,
    #[cfg(test)]
    queried_keys: std::collections::HashSet<(String, String)>,
}

impl PackageMetaMemo {
    #[allow(dead_code)]
    pub fn new() -> Self {
        Self::default()
    }

    /// Pre-index known `site-packages` / `dist-packages` roots once so Pass-2
    /// workers can fork read-only snapshots via [`Self::fork`] with zero syscalls.
    pub fn for_site_packages(roots: &[String]) -> Self {
        let mut map = HashMap::with_capacity(roots.len());
        for raw in roots {
            let norm = raw.replace('\\', "/");
            let trimmed = norm.trim_end_matches('/').to_string();
            let path = Path::new(raw);
            if path.is_dir() {
                let idx = SiteRootIndex::build(path);
                map.insert(trimmed, idx);
            }
        }
        Self {
            preindexed: Arc::new(map),
            dynamic_roots: HashMap::new(),
            fallback_map: HashMap::new(),
            #[cfg(test)]
            queried_keys: std::collections::HashSet::new(),
        }
    }

    /// Cheap worker-local handle sharing the pre-built `SiteRootIndex` map.
    pub fn fork(&self) -> Self {
        Self {
            preindexed: Arc::clone(&self.preindexed),
            dynamic_roots: HashMap::new(),
            fallback_map: HashMap::new(),
            #[cfg(test)]
            queried_keys: std::collections::HashSet::new(),
        }
    }

    /// Locate neighboring `*.dist-info` for a file under site-/dist-packages.
    /// Returns `(name, version, content_fingerprint)`.
    pub fn meta_for_path(&mut self, py_path: &Path) -> Option<PackageMetaTriple> {
        let s = py_path.to_string_lossy().replace('\\', "/");
        let (root_str, rest) = crate::cache::third_party_root_split(&s)?;
        if rest.is_empty() {
            return None;
        }
        #[cfg(test)]
        {
            let top_key = rest.split('/').next().unwrap_or(rest).to_string();
            self.queried_keys.insert((root_str.to_string(), top_key));
        }

        if let Some(idx) = self.preindexed.get(root_str) {
            if let Some(hit) = idx.lookup(rest) {
                return Some(hit.clone());
            }
            return self.fallback_unpackaged(Path::new(root_str), root_str, rest);
        }

        if !self.dynamic_roots.contains_key(root_str) {
            let built = SiteRootIndex::build(Path::new(root_str));
            self.dynamic_roots.insert(root_str.to_string(), built);
        }
        if let Some(hit) = self
            .dynamic_roots
            .get(root_str)
            .and_then(|idx| idx.lookup(rest))
        {
            return Some(hit.clone());
        }
        self.fallback_unpackaged(Path::new(root_str), root_str, rest)
    }

    fn fallback_unpackaged(
        &mut self,
        root: &Path,
        root_str: &str,
        rest: &str,
    ) -> Option<PackageMetaTriple> {
        let (key_rel, pkg_name, target_path) = match rest.split_once('/') {
            None => {
                let stem = rest.strip_suffix(".py").unwrap_or(rest);
                (rest.to_string(), stem.to_string(), root.join(rest))
            }
            Some((top, _)) => (top.to_string(), top.to_string(), root.join(top)),
        };
        if pkg_name.is_empty() {
            return None;
        }
        let key = (root_str.to_string(), key_rel);
        if let Some(cached) = self.fallback_map.get(&key) {
            return cached.clone();
        }
        let fp = package_content_fingerprint(&target_path);
        let computed = Some((pkg_name, "unknown".to_string(), fp));
        self.fallback_map.insert(key, computed.clone());
        computed
    }

    /// Number of memoized package keys (test seam).
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.queried_keys.len()
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
