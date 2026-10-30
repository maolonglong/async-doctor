//! Path matching helpers — component-aware, never bare string suffix.

use std::path::{Component, Path, PathBuf};

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
pub fn cli_path_arg(path: &Path) -> String {
    let display = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let s = display.display().to_string();
    if s.chars().any(|c| c.is_whitespace()) {
        format!("\"{s}\"")
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
}
