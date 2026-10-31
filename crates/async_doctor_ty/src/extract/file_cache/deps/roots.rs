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
    // Include site-packages so setuptools editable finders / `.pth` roots match ty.
    let site = site_packages_for_project(project_root);
    for extra in crate::search_paths::discover_extra_paths(&sys_root, &site, &existing) {
        let p = PathBuf::from(extra.as_str());
        if p.is_dir() && !roots.iter().any(|r| r == &p) {
            roots.push(p);
        }
    }
    roots
}

/// Per-file search roots for `deps_fp`, matching ty's file-relative resolution.
///
/// The importing file's parent directory is prepended so same-directory absolute
/// imports (e.g. `from helpers import mk` in `tests/test_c.py` → `tests/helpers.py`)
/// enter the closure. Without this, body/annotation edits in the neighbour miss
/// the importer's hit check.
pub fn search_roots_for_file(project_root: &Path, file_abs: &Path) -> Vec<PathBuf> {
    let mut roots = first_party_search_roots(project_root);
    let Some(parent) = file_abs.parent() else {
        return roots;
    };
    if !parent.is_dir() {
        return roots;
    }
    let parent_buf = parent.to_path_buf();
    let already = roots.iter().any(|r| {
        r == &parent_buf
            || r.canonicalize().ok().zip(parent.canonicalize().ok()).is_some_and(|(a, b)| a == b)
    });
    if !already {
        roots.insert(0, parent_buf);
    }
    roots
}

fn site_packages_for_project(project_root: &Path) -> Vec<SystemPathBuf> {
    let mut out = Vec::new();
    let mut prefixes = Vec::new();
    if let Ok(v) = std::env::var("VIRTUAL_ENV")
        && !v.is_empty()
    {
        prefixes.push(PathBuf::from(v));
    }
    for base in [project_root, project_root.parent().unwrap_or(project_root)] {
        for name in [".venv", "venv"] {
            let p = base.join(name);
            if p.is_dir() {
                prefixes.push(p);
            }
        }
    }
    for prefix in prefixes {
        // lib/pythonX.Y/site-packages or lib64/...
        let lib = prefix.join("lib");
        if let Ok(rd) = std::fs::read_dir(&lib) {
            for ent in rd.flatten() {
                let sp = ent.path().join("site-packages");
                if sp.is_dir()
                    && let Ok(sys) = SystemPathBuf::from_path_buf(sp)
                {
                    out.push(sys);
                }
            }
        }
        let sp = prefix.join("lib").join("site-packages");
        if sp.is_dir()
            && let Ok(sys) = SystemPathBuf::from_path_buf(sp)
        {
            out.push(sys);
        }
    }
    out
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

/// Walk search roots in ty order. Returns `(resolved_rel, higher_precedence_absents)`.
///
/// Higher-precedence candidate paths that do not exist (or are not in the
/// catalog) are listed so a later create at that path flips `deps_fp`.
pub fn resolve_head_with_precedence(
    head: &str,
    module_to_rel: &HashMap<String, String>,
    project_root: &Path,
    search_roots: &[PathBuf],
) -> (Option<String>, Vec<String>) {
    if head.is_empty() || head.starts_with('.') {
        return (
            resolve_head_to_rel(head, module_to_rel, project_root),
            Vec::new(),
        );
    }
    let path = head.replace('.', "/");
    let suffixes = [format!("{path}.py"), format!("{path}/__init__.py")];
    let catalog_rels: HashSet<&str> = module_to_rel.values().map(String::as_str).collect();
    let mut precedence_misses = Vec::new();
    let mut seen = HashSet::new();
    for root in search_roots {
        for suffix in &suffixes {
            let abs = root.join(suffix);
            let Some(rel) = abs_to_probe_rel(&abs, project_root, root) else {
                continue;
            };
            // First existing candidate in root order wins (matches ty search).
            if catalog_rels.contains(rel.as_str()) || abs.is_file() {
                return (Some(rel), precedence_misses);
            }
            if seen.insert(rel.clone()) {
                precedence_misses.push(rel);
            }
        }
    }
    (None, precedence_misses)
}

fn abs_to_probe_rel(abs: &Path, project_root: &Path, root: &Path) -> Option<String> {
    let proj_canon = project_root
        .canonicalize()
        .unwrap_or_else(|_| project_root.to_path_buf());
    if let Ok(r) = abs.strip_prefix(project_root) {
        return Some(r.to_string_lossy().replace('\\', "/"));
    }
    if let Ok(r) = abs.strip_prefix(&proj_canon) {
        return Some(r.to_string_lossy().replace('\\', "/"));
    }
    if let (Ok(cr), Ok(ca)) = (root.canonicalize(), abs.canonicalize()) {
        if let Ok(r) = ca.strip_prefix(&proj_canon) {
            return Some(r.to_string_lossy().replace('\\', "/"));
        }
        if let Ok(r) = ca.strip_prefix(&cr) {
            let name = cr.file_name().and_then(|n| n.to_str()).unwrap_or("ext");
            let rest = r.to_string_lossy().replace('\\', "/");
            return Some(format!("__root__/{name}/{rest}"));
        }
    }
    // Path does not exist yet — still form a project-relative probe when possible.
    let suffix = abs.strip_prefix(root).ok()?;
    if root == project_root || root.starts_with(project_root) {
        let rel = if let Ok(r) = root.strip_prefix(project_root) {
            let base = r.to_string_lossy().replace('\\', "/");
            let rest = suffix.to_string_lossy().replace('\\', "/");
            if base.is_empty() {
                rest
            } else {
                format!("{base}/{rest}")
            }
        } else {
            suffix.to_string_lossy().replace('\\', "/")
        };
        return Some(rel);
    }
    let name = root.file_name().and_then(|n| n.to_str()).unwrap_or("ext");
    let rest = suffix.to_string_lossy().replace('\\', "/");
    Some(format!("__root__/{name}/{rest}"))
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
