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
    let mut paths = Vec::new();
    let walker = WalkBuilder::new(root)
        .hidden(false)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .build();

    for entry in walker {
        let entry = entry.context("directory walk failed")?;
        if !entry.file_type().is_some_and(|ft| ft.is_file()) {
            continue;
        }
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("py") {
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
