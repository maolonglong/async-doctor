//! Path matching helpers — component-aware, never bare string suffix.

use std::path::{Component, Path, PathBuf};

/// Resolve a CLI path to an absolute path.
///
/// Bare relative names like `good.py` have parent `""`, which breaks directory
/// walks for suppressions. Absolute form makes `parent()` a real directory.
pub fn resolve_cli_path(path: &Path) -> PathBuf {
    std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Normalize separators and drop a leading `./`.
pub fn normalize_path(p: &str) -> String {
    let p = p.replace('\\', "/");
    PathBuf::from(p.trim_start_matches("./"))
        .components()
        .filter_map(|c| match c {
            Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
            Component::CurDir => None,
            other => Some(other.as_os_str().to_string_lossy().into_owned()),
        })
        .collect::<Vec<_>>()
        .join("/")
}

/// True when `a` and `b` name the same file, allowing a longer path to end with
/// the shorter path **on full path components** (`pkg/a.py` ≡ `src/pkg/a.py`),
/// but never `a.py` ≡ `data.py`.
pub fn paths_equal(a: &str, b: &str) -> bool {
    let a = normalize_path(a);
    let b = normalize_path(b);
    if a == b {
        return true;
    }
    let a_comps: Vec<&str> = a.split('/').filter(|s| !s.is_empty()).collect();
    let b_comps: Vec<&str> = b.split('/').filter(|s| !s.is_empty()).collect();
    if a_comps.is_empty() || b_comps.is_empty() {
        return false;
    }
    if a_comps.len() <= b_comps.len() {
        b_comps[b_comps.len() - a_comps.len()..] == a_comps[..]
    } else {
        a_comps[a_comps.len() - b_comps.len()..] == b_comps[..]
    }
}

/// Format a scan path for embedding in a runnable `why` command.
///
/// Prefer a path relative to the process cwd (or the path as given) — never an
/// absolute machine path, so JSON/`why` stays portable across agents/CI.
pub fn cli_path_arg(path: &Path) -> String {
    let display = relative_to_cwd(path);
    let s = if display.as_os_str().is_empty() {
        ".".to_string()
    } else {
        display.display().to_string().replace('\\', "/")
    };
    if s.chars().any(|c| c.is_whitespace()) {
        format!("\"{s}\"")
    } else {
        s
    }
}

/// Relativize `path` against the current working directory when possible.
///
/// Absolute paths canonicalize the *parent* only so a venv's `bin/python`
/// symlink is not resolved to the base interpreter.
pub fn relative_to_cwd(path: &Path) -> PathBuf {
    // Prefer the user-supplied relative path as-is (already portable).
    if !path.is_absolute() {
        if path.as_os_str().is_empty() {
            return PathBuf::from(".");
        }
        return path.to_path_buf();
    }
    let abs = absolute_keep_leaf(path);
    let Ok(cwd) = std::env::current_dir() else {
        return abs;
    };
    let cwd = cwd.canonicalize().unwrap_or(cwd);
    relative_from(&abs, &cwd)
}

/// Make `path` absolute without resolving the leaf symlink.
fn absolute_keep_leaf(path: &Path) -> PathBuf {
    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => {
            let parent_abs = parent
                .canonicalize()
                .unwrap_or_else(|_| absolute_nofollow(parent));
            match path.file_name() {
                Some(name) => parent_abs.join(name),
                None => parent_abs,
            }
        }
        _ => absolute_nofollow(path),
    }
}

fn absolute_nofollow(path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(path))
            .unwrap_or_else(|_| path.to_path_buf())
    }
}

/// Compute a relative path from `base` to `path` (both absolute).
fn relative_from(path: &Path, base: &Path) -> PathBuf {
    let path_comps: Vec<_> = path.components().collect();
    let base_comps: Vec<_> = base.components().collect();
    let common = path_comps
        .iter()
        .zip(base_comps.iter())
        .take_while(|(a, b)| a == b)
        .count();
    let mut out = PathBuf::new();
    for _ in common..base_comps.len() {
        out.push("..");
    }
    for c in &path_comps[common..] {
        out.push(*c);
    }
    if out.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        out
    }
}

/// Relativize an absolute filesystem path for human display when under cwd.
///
/// Paths that would climb out of cwd (e.g. system `/usr/...`) stay absolute so
/// the environment banner does not print awkward `../usr` forms. The leaf
/// component is never symlink-resolved (venv `bin/python` stays as such).
pub fn display_fs_path(path: &str) -> String {
    let p = Path::new(path);
    let rel = relative_to_cwd(p);
    let s = rel.display().to_string().replace('\\', "/");
    if s.starts_with("..") {
        absolute_keep_leaf(p)
            .display()
            .to_string()
            .replace('\\', "/")
    } else {
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_py_does_not_match_data_py() {
        assert!(!paths_equal("a.py", "data.py"));
        assert!(!paths_equal("data.py", "a.py"));
    }

    #[test]
    fn component_suffix_match() {
        assert!(paths_equal("pkg/a.py", "src/pkg/a.py"));
        assert!(paths_equal("src/pkg/a.py", "pkg/a.py"));
        assert!(paths_equal("a.py", "a.py"));
        assert!(!paths_equal("a.py", "pkg/data.py"));
    }

    #[test]
    fn cli_path_arg_prefers_relative() {
        let cwd = std::env::current_dir().unwrap();
        let abs = cwd.join("fixtures").join("m1_multihop");
        let arg = cli_path_arg(&abs);
        assert!(
            !arg.starts_with('/'),
            "why --path must not be absolute: {arg}"
        );
        assert!(
            arg.contains("m1_multihop"),
            "expected fixtures/m1_multihop-style relative path, got {arg}"
        );
    }

    #[test]
    fn display_fs_path_keeps_venv_python_symlink() {
        let tmp = tempfile::tempdir().unwrap();
        let venv_bin = tmp.path().join("bin");
        std::fs::create_dir_all(&venv_bin).unwrap();
        let link = venv_bin.join("python");
        // Point at a real file so is_file/symlink_metadata work.
        let target = tmp.path().join("real-python");
        std::fs::write(&target, b"").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&target, &link).unwrap();
        #[cfg(not(unix))]
        std::fs::copy(&target, &link).unwrap();

        let shown = display_fs_path(link.to_str().unwrap());
        assert!(
            shown.contains("bin/python") || shown.ends_with("python"),
            "must keep venv leaf, not resolve to real-python: {shown}"
        );
        assert!(
            !shown.contains("real-python"),
            "must not follow symlink: {shown}"
        );
    }
}
