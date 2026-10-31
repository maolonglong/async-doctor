//! Content / stub fingerprints and wide-miss fallback.

use std::path::{Path, PathBuf};

use crate::scan_cache::venv_fingerprint;

use super::{CachedFileFacts, file_content_fingerprint, short_hash};
use crate::extract::path::discover_python_stub_files;

/// Absolute miss count above which we fall back to a full cold extract.
pub const WIDE_MISS_ABS: usize = 500;

/// True when misses are wide enough that incremental extract is not worth it
/// (soundness is preserved either way; this is a perf fence).
pub fn wide_miss_fallback(miss_count: usize, catalog_len: usize) -> bool {
    if miss_count == 0 || catalog_len == 0 {
        return false;
    }
    miss_count > WIDE_MISS_ABS || miss_count.saturating_mul(2) > catalog_len
}

/// Hash sorted `rel → content_fp` of first-party `.pyi` files.
///
/// When `stub_paths` is empty, discovers stubs under `project_root`.
pub fn stub_fingerprint(project_root: &Path, stub_paths: &[PathBuf]) -> String {
    let owned: Vec<PathBuf>;
    let paths = if stub_paths.is_empty() {
        owned = discover_python_stub_files(project_root).unwrap_or_default();
        owned.as_slice()
    } else {
        stub_paths
    };
    let mut entries: Vec<(String, String)> = paths
        .iter()
        .filter_map(|p| {
            let rel = relative_to(project_root, p)?;
            let fp = file_content_fingerprint(p)?;
            Some((rel, fp))
        })
        .collect();
    entries.sort();
    let mut blob = String::from("stub-fp-v1\n");
    for (rel, fp) in entries {
        blob.push_str(&rel);
        blob.push('\t');
        blob.push_str(&fp);
        blob.push('\n');
    }
    short_hash(blob.as_bytes())
}

fn relative_to(root: &Path, path: &Path) -> Option<String> {
    let rel = path.strip_prefix(root).ok()?;
    Some(rel.to_string_lossy().replace('\\', "/"))
}

/// `(venv_fingerprint, stub_fingerprint)` for hit checks / project meta.
pub fn project_env_fingerprints(project_root: &Path) -> (String, String) {
    (
        venv_fingerprint(project_root),
        stub_fingerprint(project_root, &[]),
    )
}

/// Content / venv / stub match only (no deps_fp check).
pub fn content_venv_stub_match(
    cached: &CachedFileFacts,
    content_fp: &str,
    venv_fp: &str,
    stub_fp: &str,
) -> bool {
    cached.content_fingerprint == content_fp
        && cached.venv_fingerprint == venv_fp
        && (cached.stub_fingerprint.is_empty() || cached.stub_fingerprint == stub_fp)
}
