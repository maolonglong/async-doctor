//! Shared helpers for async_doctor_ty integration tests.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use tempfile::TempDir;

use crate::scan::scan_path_with_cache_budget;
use crate::{scan_path, scan_path_with_cache};

pub(crate) fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures")
}

#[allow(dead_code)]
pub(crate) fn report_json(path: &Path) -> String {
    let report = scan_path(path).unwrap();
    serde_json::to_string(&report).unwrap()
}

pub(crate) fn report_json_with_cache(path: &Path, cache_dir: &Path) -> String {
    let report = scan_path_with_cache(path, Some(cache_dir)).unwrap();
    serde_json::to_string(&report).unwrap()
}

/// Like [`report_json_with_cache`] with an injectable follow budget (no env vars).
pub(crate) fn report_json_with_cache_budget(
    path: &Path,
    cache_dir: &Path,
    follow_budget: usize,
) -> String {
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
///
/// Advice fixtures share [`advice_shared_venv`] — attached here so checkouts
/// without `core.symlinks` never depend on git symlinks under `fixtures/`.
pub(crate) fn clone_fixture(name: &str) -> PathBuf {
    let src = fixtures_dir().join(name);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dst =
        std::env::temp_dir().join(format!("async-doctor-fx-{name}-{}-{n}", std::process::id()));
    let _ = fs::remove_dir_all(&dst);
    copy_dir(&src, &dst);
    attach_shared_advice_venv(name, &dst);
    dst
}

/// Shared stub site-packages for advice_review_* / advice_offload_twin.
pub(crate) fn advice_shared_venv() -> PathBuf {
    fixtures_dir().join("advice_shared_venv")
}

fn attach_shared_advice_venv(name: &str, dst: &Path) {
    let needs_shared = matches!(
        name,
        "advice_offload_twin"
            | "advice_review_a"
            | "advice_review_c"
            | "advice_review_d"
            | "advice_review_g"
            | "advice_review_h"
    );
    if !needs_shared {
        return;
    }
    let shared = advice_shared_venv();
    if !shared.is_dir() {
        return;
    }
    let venv_dst = dst.join(".venv");
    if venv_dst.exists() {
        return;
    }
    copy_dir(&shared, &venv_dst);
}

pub(crate) fn copy_dir(src: &Path, dst: &Path) {
    fs::create_dir_all(dst).unwrap();
    for entry in fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let from = entry.path();
        let to = dst.join(entry.file_name());
        let meta = fs::metadata(&from).unwrap();
        if meta.is_dir() {
            copy_dir(&from, &to);
        } else {
            fs::copy(&from, to).unwrap();
        }
    }
}

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// Locate or create a venv that has the real `openai` package (for Azure advice).
pub(crate) fn real_openai_venv() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("ASYNC_DOCTOR_TEST_OPENAI_VENV") {
        let p = PathBuf::from(p);
        if p.is_dir() {
            return Some(p);
        }
    }
    [
        PathBuf::from("/tmp/async-doctor-real-openai-venv"),
        PathBuf::from("/workspace/bench/.repos/litellm/.venv"),
    ]
    .into_iter()
    .find(|candidate| {
        candidate
            .join("lib")
            .join("python3.12")
            .join("site-packages")
            .join("openai")
            .join("__init__.py")
            .is_file()
            || candidate
                .join("lib")
                .join("python3.13")
                .join("site-packages")
                .join("openai")
                .join("__init__.py")
                .is_file()
    })
}

/// Attach a real-openai-bearing venv under `dir/.venv` (copy).
pub(crate) fn attach_real_openai_venv(dir: &Path) -> bool {
    let Some(src) = real_openai_venv() else {
        return false;
    };
    let dst = dir.join(".venv");
    let _ = fs::remove_dir_all(&dst);
    copy_dir(&src, &dst);
    true
}

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
