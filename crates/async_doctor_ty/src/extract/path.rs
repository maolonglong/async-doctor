//! Project path discovery and module-name helpers.

use std::path::{Path, PathBuf};

use anyhow::Context;
use ignore::WalkBuilder;
use ruff_db::system::{SystemPath, SystemPathBuf};

pub(crate) fn absolute_utf8(path: &Path) -> anyhow::Result<SystemPathBuf> {
    let path = std::path::absolute(path)
        .with_context(|| format!("cannot resolve path `{}`", path.display()))?;
    SystemPathBuf::from_path_buf(path)
        .map_err(|p| anyhow::anyhow!("`{}` is not valid UTF-8", p.display()))
}

/// Discover first-party `.py` files under `root`, honoring `.gitignore` /
/// git excludes and skipping venv / cache / VCS trees.
///
/// Shared by extraction and CLI suppressions so ignored trees are never
/// walked twice with divergent rules.
pub fn discover_python_files(root: &Path) -> anyhow::Result<Vec<PathBuf>> {
    discover_first_party_files(root, &["py"])
}

/// Discover first-party `.py` and `.pyi` files in a single walk.
pub(crate) fn discover_python_and_stub_files(root: &Path) -> anyhow::Result<Vec<PathBuf>> {
    discover_first_party_files(root, &["py", "pyi"])
}

fn discover_first_party_files(root: &Path, extensions: &[&str]) -> anyhow::Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    let walker = WalkBuilder::new(root)
        .hidden(false)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .filter_entry(|entry| {
            if entry.depth() == 0 {
                return true;
            }
            !matches!(
                entry.file_name().to_str(),
                Some(".venv" | "venv" | "site-packages" | "__pycache__" | ".git" | "target")
            )
        })
        .build();

    for entry in walker {
        let entry = entry.context("directory walk failed")?;
        if !entry.file_type().is_some_and(|ft| ft.is_file()) {
            continue;
        }
        let path = entry.path();
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        if !extensions.contains(&ext) {
            continue;
        }
        if path.components().any(|c| {
            matches!(
                c.as_os_str().to_str(),
                Some(".venv" | "venv" | "site-packages" | "__pycache__" | ".git" | "target")
            )
        }) {
            continue;
        }
        paths.push(path.to_path_buf());
    }
    paths.sort();
    Ok(paths)
}

pub(crate) fn relative_display(root: &SystemPath, path: &SystemPath) -> String {
    path.strip_prefix(root)
        .map(|p| p.as_str().replace('\\', "/"))
        .unwrap_or_else(|_| path.as_str().replace('\\', "/"))
}

pub(crate) fn module_name_from_relative(rel: &str) -> String {
    let mut path = rel.replace('\\', "/");
    if let Some(stripped) = path.strip_suffix(".py") {
        path = stripped.to_string();
    }
    if let Some(stripped) = path.strip_suffix("/__init__") {
        path = stripped.to_string();
    }
    if path == "__init__" || path.is_empty() {
        return String::new();
    }
    path.replace('/', ".")
}
