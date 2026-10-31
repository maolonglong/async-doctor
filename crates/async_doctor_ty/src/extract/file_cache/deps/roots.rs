//! First-party search roots and import-head → rel resolution.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use ruff_db::system::SystemPathBuf;

use crate::extract::path::module_name_from_relative;

use super::DepCatalogEntry;

pub fn first_party_search_roots(project_root: &Path) -> Vec<PathBuf> {
    let mut roots = vec![project_root.to_path_buf()];
    let src_layout = project_root.join("src");
    if src_layout.is_dir() {
        roots.push(src_layout);
    }
    if project_root.join("__init__.py").is_file()
        && let Some(parent) = project_root.parent()
    {
        let parent_buf = parent.to_path_buf();
        if !roots.iter().any(|r| r == &parent_buf) {
            roots.push(parent_buf);
        }
    }
    let Ok(sys_root) = SystemPathBuf::from_path_buf(project_root.to_path_buf()) else {
        return roots;
    };
    let existing: Vec<SystemPathBuf> = roots
        .iter()
        .filter_map(|r| SystemPathBuf::from_path_buf(r.clone()).ok())
        .collect();
    let site: Vec<SystemPathBuf> = Vec::new();
    for extra in crate::search_paths::discover_extra_paths(&sys_root, &site, &existing) {
        let p = PathBuf::from(extra.as_str());
        if p.is_dir() && !roots.iter().any(|r| r == &p) {
            roots.push(p);
        }
    }
    roots
}

/// Module name of `abs` when imported via search root `root` (exact strip).
pub fn module_under_search_root(root: &Path, abs: &Path) -> Option<String> {
    let rel = abs.strip_prefix(root).ok()?;
    let rel = rel.to_string_lossy().replace('\\', "/");
    if rel.is_empty() {
        return None;
    }
    Some(module_name_from_relative(&rel))
}

/// Prefer the longest matching search-root prefix (e.g. `src/` over project).
pub fn importable_module(abs: &Path, roots: &[PathBuf]) -> String {
    let mut best: Option<(usize, String)> = None;
    for root in roots {
        let Ok(canon_root) = root.canonicalize() else {
            if let Some(m) = module_under_search_root(root, abs) {
                let len = root.as_os_str().len();
                if best.as_ref().is_none_or(|(l, _)| len > *l) {
                    best = Some((len, m));
                }
            }
            continue;
        };
        let abs_c = abs.canonicalize().unwrap_or_else(|_| abs.to_path_buf());
        if let Some(m) = module_under_search_root(&canon_root, &abs_c) {
            let len = canon_root.as_os_str().len();
            if best.as_ref().is_none_or(|(l, _)| len > *l) {
                best = Some((len, m));
            }
        }
    }
    best.map(|(_, m)| m).unwrap_or_default()
}

/// Build `module_name (+ search-root / package aliases) → rel` for exact resolution.
pub fn build_module_to_rel(
    catalog: &[DepCatalogEntry],
    scan_root: &Path,
) -> HashMap<String, String> {
    let roots = first_party_search_roots(scan_root);
    let mut out = HashMap::with_capacity(catalog.len().saturating_mul(4));
    for e in catalog {
        for alias in module_name_aliases(scan_root, &e.module) {
            out.insert(alias, e.rel.clone());
        }
        for root in &roots {
            if let Some(m) = module_under_search_root(root, &e.abs)
                && !m.is_empty()
            {
                out.insert(m, e.rel.clone());
            }
            // Also try canonical paths when abs is already canonicalized.
            if let (Ok(cr), Ok(ca)) = (root.canonicalize(), e.abs.canonicalize())
                && let Some(m) = module_under_search_root(&cr, &ca)
                && !m.is_empty()
            {
                out.insert(m, e.rel.clone());
            }
        }
    }
    out
}

/// Package-root aliases: when scanning `pkg/` itself, `util` is also `pkg.util`.
pub fn module_name_aliases(scan_root: &Path, module: &str) -> Vec<String> {
    let mut names = Vec::with_capacity(2);
    names.push(module.to_string());
    if scan_root.join("__init__.py").is_file()
        && let Some(pkg) = scan_root.file_name().and_then(|n| n.to_str())
        && !pkg.is_empty()
        && pkg != "."
        && pkg != ".."
    {
        if module.is_empty() {
            if !names.iter().any(|n| n == pkg) {
                names.push(pkg.to_string());
            }
        } else {
            let aliased = format!("{pkg}.{module}");
            if !names.iter().any(|n| n == &aliased) {
                names.push(aliased);
            }
        }
    }
    names
}

/// Exact catalog / alias match only — **no** parent fallback.
pub fn resolve_head_to_rel(
    head: &str,
    module_to_rel: &HashMap<String, String>,
    _scan_root: &Path,
) -> Option<String> {
    module_to_rel.get(head).cloned()
}

/// Project-relative absent probes under every first-party search root.
///
/// `from pkg.newm import f` with a `src/` layout probes `src/pkg/newm.py`
/// (and `__init__`), not only `<root>/pkg/newm.py`.
pub fn absent_probe_rels(head: &str, project_root: &Path, roots: &[PathBuf]) -> Vec<String> {
    if head.is_empty() || head.starts_with('.') {
        return Vec::new();
    }
    let path = head.replace('.', "/");
    let suffixes = [format!("{path}.py"), format!("{path}/__init__.py")];
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let proj_canon = project_root
        .canonicalize()
        .unwrap_or_else(|_| project_root.to_path_buf());
    for root in roots {
        for suffix in &suffixes {
            let abs = root.join(suffix);
            let rel = if let Ok(r) = abs.strip_prefix(project_root) {
                r.to_string_lossy().replace('\\', "/")
            } else if let Ok(r) = abs.strip_prefix(&proj_canon) {
                r.to_string_lossy().replace('\\', "/")
            } else if let (Ok(cr), Ok(ca)) = (root.canonicalize(), abs.canonicalize()) {
                match ca.strip_prefix(&proj_canon) {
                    Ok(r) => r.to_string_lossy().replace('\\', "/"),
                    Err(_) => match ca.strip_prefix(&cr) {
                        // Root outside the project (editable): keep root-relative
                        // marker prefixed so it cannot collide with in-tree paths.
                        Ok(r) => {
                            let name = cr.file_name().and_then(|n| n.to_str()).unwrap_or("ext");
                            let rest = r.to_string_lossy().replace('\\', "/");
                            format!("__root__/{name}/{rest}")
                        }
                        Err(_) => continue,
                    },
                }
            } else {
                continue;
            };
            if seen.insert(rel.clone()) {
                out.push(rel);
            }
        }
    }
    out
}
