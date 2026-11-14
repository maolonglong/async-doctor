//! Resolve async-twin symbols against the scanned venv's site-packages.
//!
//! Advice requires the twin symbol (and its `needs`) to be present in the
//! environment. We check the filesystem under the active venv rather than
//! re-entering ty's module resolver (keeps extract/cache seams small).

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use async_doctor_rules::twin_resolution_symbols;

use crate::env_discovery::discover_site_packages_for_path;

/// Populate `resolved_symbols` for every twin / needs entry that exists in the
/// active Python environment for `scan_root`.
#[cfg(test)]
pub fn collect_resolved_twin_symbols(scan_root: &Path) -> BTreeSet<String> {
    collect_resolved_twin_symbols_with_python(scan_root, None)
}

/// Populate `resolved_symbols` using an optional explicit `--python` environment.
pub fn collect_resolved_twin_symbols_with_python(
    scan_root: &Path,
    python: Option<&Path>,
) -> BTreeSet<String> {
    let site_roots = site_packages_roots(scan_root, python);
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

fn site_packages_roots(scan_root: &Path, python: Option<&Path>) -> Vec<PathBuf> {
    discover_site_packages_for_path(scan_root, python).2
}

fn symbol_available(site_roots: &[PathBuf], symbol: &str) -> bool {
    let parts: Vec<&str> = symbol.split('.').collect();
    if parts.is_empty() {
        return false;
    }
    // Package-only needs (e.g. `aiohttp`): directory, `.py`/`.pyi`, or compiled extension.
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

fn find_module_in_dir(dir: &Path, name: &str) -> Option<PathBuf> {
    let sub = dir.join(name);
    if sub.is_dir() {
        return Some(sub);
    }
    let py = dir.join(format!("{name}.py"));
    if py.is_file() {
        return Some(py);
    }
    let pyi = dir.join(format!("{name}.pyi"));
    if pyi.is_file() {
        return Some(pyi);
    }
    for ext in ["so", "pyd"] {
        let exact = dir.join(format!("{name}.{ext}"));
        if exact.is_file() {
            return Some(exact);
        }
    }
    let prefix = format!("{name}.");
    let entries = fs::read_dir(dir).ok()?;
    for ent in entries.flatten() {
        let p = ent.path();
        if !p.is_file() {
            continue;
        }
        let Some(fname) = p.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if fname.starts_with(&prefix) && (fname.ends_with(".so") || fname.ends_with(".pyd")) {
            return Some(p);
        }
    }
    None
}

fn package_root(site_roots: &[PathBuf], name: &str) -> Option<PathBuf> {
    site_roots
        .iter()
        .find_map(|root| find_module_in_dir(root, name))
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
        cur = find_module_in_dir(&cur, part)?;
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
        let ext = parent.extension().and_then(|e| e.to_str());
        if matches!(ext, Some("py" | "pyi")) {
            let mut out = vec![parent.to_path_buf()];
            if ext == Some("py") {
                let stub = parent.with_extension("pyi");
                if stub.is_file() {
                    out.push(stub);
                }
            }
            return out;
        }
        if let Some(dir) = parent.parent()
            && let Some(fname) = parent.file_name().and_then(|n| n.to_str())
            && let Some((stem, _)) = fname.split_once('.')
        {
            let stub = dir.join(format!("{stem}.pyi"));
            if stub.is_file() {
                return vec![stub];
            }
        }
        return Vec::new();
    }
    let mut files = Vec::new();
    for candidate in [parent.join("__init__.py"), parent.join("__init__.pyi")] {
        if candidate.is_file() {
            files.push(candidate);
        }
    }
    // Shallow sibling modules (httpx/_client.py, _client.pyi) — definitions, not every import.
    if let Ok(entries) = fs::read_dir(parent) {
        for ent in entries.flatten() {
            let p = ent.path();
            let ext = p.extension().and_then(|e| e.to_str());
            if matches!(ext, Some("py") | Some("pyi")) && !files.contains(&p) {
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
    // Absolute within site-/dist-packages or PDM __pypackages__/<ver>/lib.
    let mut cur = parent_dir.to_path_buf();
    loop {
        let fname = cur.file_name().and_then(|s| s.to_str());
        let is_pdm_lib = fname == Some("lib")
            && cur
                .parent()
                .and_then(Path::parent)
                .and_then(|p| p.file_name())
                .and_then(|s| s.to_str())
                == Some("__pypackages__");
        if matches!(fname, Some("site-packages" | "dist-packages")) || is_pdm_lib {
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
        if cur.is_file() {
            return None;
        }
        cur = find_module_in_dir(&cur, part)?;
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

    #[test]
    fn detects_pyi_stubs_compiled_extensions_and_rejects_missing_leaf_in_submodule_file() {
        let dir = std::env::temp_dir().join(format!(
            "async-doctor-twin-ext-stub-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = fs::remove_dir_all(&dir);
        let sp = dir
            .join(".venv")
            .join("lib")
            .join("python3.12")
            .join("site-packages");
        fs::create_dir_all(&sp).unwrap();

        // 1. Top-level compiled extension (.so / .pyd) satisfies package need (`aiohttp`).
        fs::write(sp.join("aiohttp.cpython-312-darwin.so"), b"\x7fELFstub").unwrap();
        // 2. Stub-only subpackage (`grpc/aio/__init__.pyi`) satisfies `grpc.aio`.
        fs::create_dir_all(sp.join("grpc").join("aio")).unwrap();
        fs::write(sp.join("grpc").join("__init__.pyi"), "").unwrap();
        fs::write(
            sp.join("grpc").join("aio").join("__init__.pyi"),
            "class Channel: ...\n",
        )
        .unwrap();
        // 3. Re-export pointing at a `.pyi` stub alongside a `.pyd` extension.
        fs::create_dir_all(sp.join("httpx")).unwrap();
        fs::write(
            sp.join("httpx").join("__init__.pyi"),
            "from ._accel import AsyncClient as AsyncClient\n",
        )
        .unwrap();
        fs::write(
            sp.join("httpx").join("_accel.cp312-win_amd64.pyd"),
            b"MZstub",
        )
        .unwrap();
        fs::write(
            sp.join("httpx").join("_accel.pyi"),
            "class AsyncClient: ...\n",
        )
        .unwrap();
        // 4. Multi-part symbol where intermediate `sqlalchemy/ext/asyncio.py` is a file
        //    that does NOT define `AsyncSession` — must NOT falsely report `AsyncSession`!
        fs::create_dir_all(sp.join("sqlalchemy").join("ext")).unwrap();
        fs::write(sp.join("sqlalchemy").join("__init__.py"), "").unwrap();
        fs::write(sp.join("sqlalchemy").join("ext").join("__init__.py"), "").unwrap();
        fs::write(
            sp.join("sqlalchemy").join("ext").join("asyncio.py"),
            "class SomethingElse: pass\n",
        )
        .unwrap();

        let resolved = collect_resolved_twin_symbols(&dir);
        assert!(
            resolved.contains("aiohttp"),
            "compiled .so package should resolve: {resolved:?}"
        );
        assert!(
            resolved.contains("grpc.aio"),
            "pyi subpackage should resolve: {resolved:?}"
        );
        assert!(
            resolved.contains("httpx.AsyncClient"),
            "re-export from .pyi + .pyd should resolve: {resolved:?}"
        );
        assert!(
            !resolved.contains("sqlalchemy.ext.asyncio.AsyncSession"),
            "submodule file missing AsyncSession must NOT resolve: {resolved:?}"
        );

        let _ = fs::remove_dir_all(&dir);
    }
}
