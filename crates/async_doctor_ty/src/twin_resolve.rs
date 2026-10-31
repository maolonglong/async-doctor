//! Resolve async-twin symbols against the scanned venv's site-packages.
//!
//! Advice requires the twin symbol (and its `needs`) to be present in the
//! environment. We check the filesystem under the active venv rather than
//! re-entering ty's module resolver (keeps extract/cache seams small).

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use async_doctor_rules::twin_resolution_symbols;

/// Populate `resolved_symbols` for every twin / needs entry that exists in the
/// active venv (or in-project `.venv` / `venv` under `scan_root`).
pub fn collect_resolved_twin_symbols(scan_root: &Path) -> BTreeSet<String> {
    let site_roots = site_packages_roots(scan_root);
    let mut out = BTreeSet::new();
    if site_roots.is_empty() {
        return out;
    }
    for symbol in twin_resolution_symbols() {
        if symbol_available(&site_roots, symbol) {
            out.insert(symbol.to_string());
        }
    }
    out
}

fn site_packages_roots(scan_root: &Path) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    let mut prefixes = Vec::new();
    if let Ok(v) = std::env::var("VIRTUAL_ENV")
        && !v.is_empty()
    {
        let p = PathBuf::from(v);
        if p.is_dir() {
            prefixes.push(p);
        }
    }
    for base in [scan_root, scan_root.parent().unwrap_or(scan_root)] {
        for name in [".venv", "venv"] {
            let p = base.join(name);
            if p.is_dir() {
                prefixes.push(p);
            }
        }
    }
    prefixes.sort();
    prefixes.dedup();
    for prefix in prefixes {
        let lib = prefix.join("lib");
        if let Ok(entries) = fs::read_dir(&lib) {
            for ent in entries.flatten() {
                let name = ent.file_name();
                let name = name.to_string_lossy();
                if name.starts_with("python") {
                    let sp = ent.path().join("site-packages");
                    if sp.is_dir() {
                        roots.push(sp);
                    }
                }
            }
        }
        let sp = prefix.join("Lib").join("site-packages");
        if sp.is_dir() {
            roots.push(sp);
        }
    }
    roots.sort();
    roots.dedup();
    roots
}

fn symbol_available(site_roots: &[PathBuf], symbol: &str) -> bool {
    let parts: Vec<&str> = symbol.split('.').collect();
    if parts.is_empty() {
        return false;
    }
    // Package-only needs (e.g. `aiohttp`): directory or module file exists.
    if parts.len() == 1 {
        return package_root(site_roots, parts[0]).is_some();
    }
    // Module path like `redis.asyncio`: walk packages, final may be package or module.
    if let Some(path) = walk_module_path(site_roots, &parts) {
        return path.is_dir() || path.is_file();
    }
    // Class attribute like `httpx.AsyncClient` / `openai.AsyncOpenAI`:
    // parent module/package must exist and mention the final name.
    let (parent, name) = parts.split_at(parts.len() - 1);
    let Some(parent_path) = walk_module_path(site_roots, parent) else {
        return false;
    };
    let needle = name[0];
    name_exported_in(&parent_path, needle)
}

fn package_root(site_roots: &[PathBuf], name: &str) -> Option<PathBuf> {
    for root in site_roots {
        let dir = root.join(name);
        if dir.is_dir() {
            return Some(dir);
        }
        let py = root.join(format!("{name}.py"));
        if py.is_file() {
            return Some(py);
        }
    }
    None
}

fn walk_module_path(site_roots: &[PathBuf], parts: &[&str]) -> Option<PathBuf> {
    if parts.is_empty() {
        return None;
    }
    let mut cur = package_root(site_roots, parts[0])?;
    for part in &parts[1..] {
        if cur.is_file() {
            return None;
        }
        let next_dir = cur.join(part);
        if next_dir.is_dir() {
            cur = next_dir;
            continue;
        }
        let next_py = cur.join(format!("{part}.py"));
        if next_py.is_file() {
            return Some(next_py);
        }
        return None;
    }
    Some(cur)
}

fn name_exported_in(parent: &Path, name: &str) -> bool {
    let files = if parent.is_file() {
        vec![parent.to_path_buf()]
    } else {
        let mut files = Vec::new();
        for candidate in [
            parent.join("__init__.py"),
            parent.join("__init__.pyi"),
            parent.join(format!("{name}.py")),
            parent.join(format!("{name}.pyi")),
        ] {
            if candidate.is_file() {
                files.push(candidate);
            }
        }
        // Also scan shallow sibling modules (httpx/_client.py).
        if let Ok(entries) = fs::read_dir(parent) {
            for ent in entries.flatten() {
                let p = ent.path();
                if p.extension().and_then(|e| e.to_str()) == Some("py")
                    || p.extension().and_then(|e| e.to_str()) == Some("pyi")
                {
                    files.push(p);
                }
            }
        }
        files
    };
    // Prefer definition-like tokens so docstrings mentioning the name do not
    // count as an export (e.g. "no AsyncClient" in a comment).
    let patterns = [
        format!("class {name}"),
        format!("class {name}:"),
        format!("class {name}("),
        format!("def {name}("),
        format!("{name} ="),
        format!("'{name}'"),
        format!("\"{name}\""),
        format!(" {name},"),
        format!("({name},"),
        format!("[{name},"),
        format!("{{{name},"),
    ];
    for file in files {
        let Ok(text) = fs::read_to_string(&file) else {
            continue;
        };
        if patterns.iter().any(|p| text.contains(p.as_str())) {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn detects_httpx_async_client_in_fixture_layout() {
        let dir = std::env::temp_dir().join(format!(
            "async-doctor-twin-resolve-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        let sp = dir
            .join(".venv")
            .join("lib")
            .join("python3.12")
            .join("site-packages")
            .join("httpx");
        fs::create_dir_all(&sp).unwrap();
        fs::write(sp.join("__init__.py"), "from ._client import AsyncClient, Client\n").unwrap();
        fs::write(
            sp.join("_client.py"),
            "class Client: ...\nclass AsyncClient: ...\n",
        )
        .unwrap();
        let resolved = collect_resolved_twin_symbols(&dir);
        assert!(
            resolved.contains("httpx.AsyncClient"),
            "resolved={resolved:?}"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn needs_aiohttp_absent() {
        let dir = std::env::temp_dir().join(format!(
            "async-doctor-twin-needs-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        let sp = dir
            .join(".venv")
            .join("lib")
            .join("python3.12")
            .join("site-packages");
        fs::create_dir_all(sp.join("elasticsearch")).unwrap();
        fs::write(
            sp.join("elasticsearch").join("__init__.py"),
            "class Elasticsearch: ...\nclass AsyncElasticsearch: ...\n",
        )
        .unwrap();
        let resolved = collect_resolved_twin_symbols(&dir);
        assert!(resolved.contains("elasticsearch.AsyncElasticsearch"));
        assert!(!resolved.contains("aiohttp"));
        let _ = fs::remove_dir_all(&dir);
    }
}
