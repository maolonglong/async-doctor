//! Resolve async-twin symbols against the scanned venv's site-packages.
//!
//! Advice requires the twin symbol (and its `needs`) to be present in the
//! environment. We check the filesystem under the active venv rather than
//! re-entering ty's module resolver (keeps extract/cache seams small).

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use async_doctor_rules::twin_resolution_symbols;

use crate::scan_cache::{resolve_venv_prefix, site_packages_roots_for_prefix};

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
    // Prefer a single active prefix (same policy as scan_cache): VIRTUAL_ENV,
    // else in-project `.venv`/`venv`. Do not union multiple venvs — that would
    // let a foreign VIRTUAL_ENV satisfy `needs` for an isolated fixture.
    resolve_venv_prefix(scan_root)
        .map(|prefix| site_packages_roots_for_prefix(&prefix))
        .unwrap_or_default()
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
    // Class attribute like `httpx.AsyncClient` / `openai.AsyncAzureOpenAI`:
    // parent module/package must export the final name (class or re-export).
    let (parent, name) = parts.split_at(parts.len() - 1);
    let Some(parent_path) = walk_module_path(site_roots, parent) else {
        return false;
    };
    let needle = name[0];
    name_exported_in(&parent_path, needle, 0)
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
        let pyi = root.join(format!("{name}.pyi"));
        if pyi.is_file() {
            return Some(pyi);
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

/// True when `name` is defined or re-exported from `parent` (package dir or module file).
fn name_exported_in(parent: &Path, name: &str, depth: u8) -> bool {
    if depth > 8 {
        return false;
    }
    let files = init_and_sibling_py_files(parent);
    for file in &files {
        let Ok(text) = fs::read_to_string(file) else {
            continue;
        };
        if defines_name_locally(&text, name) {
            return true;
        }
        if let Some(target) = resolve_reexport_target(file, &text, name)
            && name_exported_in(&target, name, depth + 1)
        {
            return true;
        }
    }
    false
}

fn init_and_sibling_py_files(parent: &Path) -> Vec<PathBuf> {
    if parent.is_file() {
        return vec![parent.to_path_buf()];
    }
    let mut files = Vec::new();
    for candidate in [parent.join("__init__.py"), parent.join("__init__.pyi")] {
        if candidate.is_file() {
            files.push(candidate);
        }
    }
    // Shallow sibling modules (httpx/_client.py) — definitions, not every import.
    if let Ok(entries) = fs::read_dir(parent) {
        for ent in entries.flatten() {
            let p = ent.path();
            let ext = p.extension().and_then(|e| e.to_str());
            if matches!(ext, Some("py") | Some("pyi")) {
                files.push(p);
            }
        }
    }
    files
}

fn defines_name_locally(text: &str, name: &str) -> bool {
    // Prefer definition-like tokens so docstrings mentioning the name do not
    // count as an export (e.g. "no AsyncClient" in a comment).
    let patterns = [
        format!("class {name}"),
        format!("class {name}:"),
        format!("class {name}("),
        format!("def {name}("),
        format!("{name} ="),
    ];
    patterns.iter().any(|p| text.contains(p.as_str()))
}

/// If `name` is imported into this file, return the module path it comes from
/// (package dir or `.py` file) so the caller can look for a local definition.
///
/// Handles `from .lib.azure import AzureOpenAI as AzureOpenAI, AsyncAzureOpenAI as …`,
/// parenthesized multi-line imports, and plain `from x import Name`.
fn resolve_reexport_target(file: &Path, text: &str, name: &str) -> Option<PathBuf> {
    let module_path = import_module_path_for_name(text, name)?;
    resolve_relative_module(file, &module_path)
}

fn import_module_path_for_name(text: &str, name: &str) -> Option<String> {
    // Normalize so parenthesized multi-line imports become single-line-ish.
    let flat = flatten_python_imports(text);
    for line in flat.lines() {
        let line = line.trim();
        if !line.starts_with("from ") || !line.contains(" import ") {
            continue;
        }
        let Some((from_part, import_part)) = line.split_once(" import ") else {
            continue;
        };
        if !import_list_binds_name(import_part, name) {
            continue;
        }
        let module = from_part.trim().strip_prefix("from ")?.trim();
        if module.is_empty() {
            continue;
        }
        return Some(module.to_string());
    }
    None
}

fn flatten_python_imports(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut paren = 0i32;
    for ch in text.chars() {
        match ch {
            '(' => {
                paren += 1;
                out.push(' ');
            }
            ')' => {
                paren = paren.saturating_sub(1);
                out.push(' ');
            }
            '\n' | '\r' if paren > 0 => out.push(' '),
            _ => out.push(ch),
        }
    }
    out
}

fn import_list_binds_name(import_part: &str, name: &str) -> bool {
    let cleaned = import_part
        .trim()
        .trim_start_matches('(')
        .trim_end_matches(')')
        .trim()
        .trim_end_matches('\\')
        .trim();
    for item in cleaned.split(',') {
        let item = item.trim();
        if item.is_empty() {
            continue;
        }
        // `Name` / `Name as Name` / `orig as Name`
        if let Some((_orig, alias)) = item.split_once(" as ") {
            if alias.trim() == name {
                return true;
            }
        } else if item == name {
            return true;
        }
    }
    false
}

fn resolve_relative_module(from_file: &Path, module_path: &str) -> Option<PathBuf> {
    let parent_dir = if from_file.is_file() {
        from_file.parent()?
    } else {
        from_file
    };
    // Relative: `.lib.azure` / `..foo` / `.`
    if let Some(rest) = module_path.strip_prefix('.') {
        // Count leading dots for parent hops; the first strip consumed one.
        let mut dots = 1usize;
        let mut body = rest;
        while let Some(b) = body.strip_prefix('.') {
            dots += 1;
            body = b;
        }
        let mut base = parent_dir.to_path_buf();
        // `from .lib.azure` in pkg/__init__.py → pkg/; one leading dot = same package.
        // Extra dots walk up.
        for _ in 1..dots {
            base = base.parent()?.to_path_buf();
        }
        if body.is_empty() {
            return Some(base);
        }
        return walk_from_base(&base, body);
    }
    // Absolute within site-packages: walk up to site-packages then down.
    let mut cur = parent_dir.to_path_buf();
    loop {
        if cur.file_name().and_then(|s| s.to_str()) == Some("site-packages")
            || cur.file_name().and_then(|s| s.to_str()) == Some("dist-packages")
        {
            return walk_from_base(&cur, module_path);
        }
        if !cur.pop() {
            break;
        }
    }
    None
}

fn walk_from_base(base: &Path, dotted: &str) -> Option<PathBuf> {
    let mut cur = base.to_path_buf();
    for part in dotted.split('.').filter(|p| !p.is_empty()) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn detects_httpx_async_client_in_fixture_layout() {
        let dir =
            std::env::temp_dir().join(format!("async-doctor-twin-resolve-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let sp = dir
            .join(".venv")
            .join("lib")
            .join("python3.12")
            .join("site-packages")
            .join("httpx");
        fs::create_dir_all(&sp).unwrap();
        fs::write(
            sp.join("__init__.py"),
            "from ._client import AsyncClient, Client\n",
        )
        .unwrap();
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
    fn detects_openai_async_azure_via_as_reexport() {
        // Real openai: `from .lib.azure import AzureOpenAI as AzureOpenAI, AsyncAzureOpenAI as AsyncAzureOpenAI`
        let dir =
            std::env::temp_dir().join(format!("async-doctor-twin-azure-as-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let sp = dir
            .join(".venv")
            .join("lib")
            .join("python3.12")
            .join("site-packages")
            .join("openai");
        fs::create_dir_all(sp.join("lib")).unwrap();
        fs::write(
            sp.join("__init__.py"),
            concat!(
                "from .lib.azure import AzureOpenAI as AzureOpenAI, AsyncAzureOpenAI as AsyncAzureOpenAI\n",
                "class OpenAI: pass\n",
                "class AsyncOpenAI: pass\n",
            ),
        )
        .unwrap();
        fs::write(
            sp.join("lib").join("azure.py"),
            "class AzureOpenAI: pass\nclass AsyncAzureOpenAI: pass\n",
        )
        .unwrap();
        let resolved = collect_resolved_twin_symbols(&dir);
        assert!(
            resolved.contains("openai.AsyncAzureOpenAI"),
            "AsyncAzureOpenAI via `as` re-export must resolve: {resolved:?}"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn detects_parenthesized_multiline_reexport() {
        let dir =
            std::env::temp_dir().join(format!("async-doctor-twin-paren-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let sp = dir
            .join(".venv")
            .join("lib")
            .join("python3.12")
            .join("site-packages")
            .join("openai");
        fs::create_dir_all(sp.join("lib")).unwrap();
        fs::write(
            sp.join("__init__.py"),
            concat!(
                "from .lib.azure import (\n",
                "    AzureOpenAI as AzureOpenAI,\n",
                "    AsyncAzureOpenAI as AsyncAzureOpenAI,\n",
                ")\n",
            ),
        )
        .unwrap();
        fs::write(
            sp.join("lib").join("azure.py"),
            "class AzureOpenAI: pass\nclass AsyncAzureOpenAI: pass\n",
        )
        .unwrap();
        let resolved = collect_resolved_twin_symbols(&dir);
        assert!(
            resolved.contains("openai.AsyncAzureOpenAI"),
            "parenthesized re-export must resolve: {resolved:?}"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn needs_aiohttp_absent() {
        let dir =
            std::env::temp_dir().join(format!("async-doctor-twin-needs-{}", std::process::id()));
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
