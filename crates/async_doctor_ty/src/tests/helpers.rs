//! Shared helpers for async_doctor_ty integration tests.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use tempfile::TempDir;

use crate::sitepkgs::clear_package_meta_memo;
use crate::{scan_path, scan_path_with_cache, scan_path_with_cache_budget};

pub(crate) fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures")
}

#[allow(dead_code)]
pub(crate) fn report_json(path: &Path) -> String {
    let report = scan_path(path).unwrap();
    serde_json::to_string(&report).unwrap()
}

pub(crate) fn report_json_with_cache(path: &Path, cache_dir: &Path) -> String {
    clear_package_meta_memo();
    let report = scan_path_with_cache(path, Some(cache_dir)).unwrap();
    serde_json::to_string(&report).unwrap()
}

/// Like [`report_json_with_cache`] with an injectable follow budget (no env vars).
pub(crate) fn report_json_with_cache_budget(
    path: &Path,
    cache_dir: &Path,
    follow_budget: usize,
) -> String {
    clear_package_meta_memo();
    let report = scan_path_with_cache_budget(path, Some(cache_dir), follow_budget).unwrap();
    serde_json::to_string(&report).unwrap()
}

/// Isolated cache root — includes pid so parallel cargo-test *processes* cannot
/// clobber each other (in-process counter alone is not enough).
pub(crate) fn fresh_cache_dir(label: &str) -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let cache =
        std::env::temp_dir().join(format!("async-doctor-{label}-{}-{n}", std::process::id()));
    let _ = fs::remove_dir_all(&cache);
    fs::create_dir_all(&cache).unwrap();
    cache
}

/// Copy a fixture tree into an isolated temp dir (pid + counter; safe under
/// multi-process `cargo test`).
pub(crate) fn clone_fixture(name: &str) -> PathBuf {
    let src = fixtures_dir().join(name);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dst =
        std::env::temp_dir().join(format!("async-doctor-fx-{name}-{}-{n}", std::process::id()));
    let _ = fs::remove_dir_all(&dst);
    copy_dir(&src, &dst);
    dst
}

pub(crate) fn copy_dir(src: &Path, dst: &Path) {
    fs::create_dir_all(dst).unwrap();
    for entry in fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let to = dst.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &to);
        } else {
            fs::copy(entry.path(), to).unwrap();
        }
    }
}

static COUNTER: AtomicU64 = AtomicU64::new(0);

pub(crate) fn temp_project(files: &[(&str, &str)]) -> PathBuf {
    // TempDir creates a unique path (pid+random); keep the PathBuf and leak the
    // guard so the directory survives until process exit (tests remove explicitly).
    let tmp = TempDir::new().expect("temp dir");
    let dir = tmp.path().to_path_buf();
    // Persist beyond TempDir drop by forgetting the guard after materializing files.
    for (name, content) in files {
        let path = dir.join(name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, content).unwrap();
    }
    let _ = tmp.keep();
    dir
}
