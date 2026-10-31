//! Shared helpers for incremental file-cache identity tests.

use std::fs;
use std::path::{Path, PathBuf};

use super::super::helpers::{fresh_cache_dir, report_json_with_cache, temp_project};
use crate::cache::CACHE_SCHEMA_VERSION;

pub(super) fn wipe_scan_cache(cache: &Path) {
    let tool = env!("CARGO_PKG_VERSION");
    let scan_dir = cache
        .join(format!("v{CACHE_SCHEMA_VERSION}"))
        .join(tool)
        .join("scan");
    if scan_dir.is_dir() {
        let _ = fs::remove_dir_all(&scan_dir);
    }
}

pub(super) fn files_cache_dir(cache: &Path) -> PathBuf {
    let tool = env!("CARGO_PKG_VERSION");
    cache
        .join(format!("v{CACHE_SCHEMA_VERSION}"))
        .join(tool)
        .join("files")
}

/// Incremental extract (scan cache wiped) must equal a fresh cold on a new cache root.
pub(super) fn incremental_vs_cold(dir: &Path, cache: &Path, label: &str) {
    wipe_scan_cache(cache);
    let incremental = report_json_with_cache(dir, cache);
    let cache2 = fresh_cache_dir(&format!("{label}-cold"));
    let cold = report_json_with_cache(dir, &cache2);
    assert_eq!(
        incremental, cold,
        "{label}: incremental must be byte-identical to cold\nincr={incremental}\ncold={cold}"
    );
    let _ = fs::remove_dir_all(&cache2);
}

pub(super) fn walkdir_json(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = fs::read_dir(&dir) else {
            continue;
        };
        for ent in rd.flatten() {
            let p = ent.path();
            if p.is_dir() {
                stack.push(p);
                continue;
            }
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name.ends_with(".json") || name.ends_with(".json.gz") {
                out.push(p);
            }
        }
    }
    out
}

pub(super) fn read_slot_json(path: &Path) -> String {
    use std::io::Read;
    if path.extension().and_then(|e| e.to_str()) == Some("gz") {
        let raw = fs::read(path).unwrap();
        let mut dec = flate2::read::GzDecoder::new(raw.as_slice());
        let mut s = String::new();
        dec.read_to_string(&mut s).unwrap();
        s
    } else {
        fs::read_to_string(path).unwrap()
    }
}

/// Default `scen.sh` / `scen2.sh` package tree (`base()`).
pub(super) fn scen_base() -> PathBuf {
    temp_project(&[
        ("pkg/__init__.py", ""),
        (
            "pkg/app.py",
            "from pkg.mid import helper\nfrom pkg import util\nasync def handler():\n    helper()\nasync def h2():\n    util.work()\n",
        ),
        ("pkg/mid.py", "from pkg.low import helper\n"),
        (
            "pkg/low.py",
            "import time\ndef helper():\n    time.sleep(1)\n",
        ),
        (
            "pkg/util.py",
            "import time\ndef work():\n    time.sleep(1)\n",
        ),
    ])
}

/// Copy a temp project into a directory named `pkg` under a unique parent so
/// absolute self-imports (`from pkg…`) resolve when the scan root is the
/// package directory itself (`absroot`).
pub(super) fn as_package_dir_root(files: &[(&str, &str)], label: &str) -> (PathBuf, PathBuf) {
    let staging = temp_project(files);
    let parent = staging.parent().unwrap().join(format!(
        "async-doctor-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let pkg = parent.join("pkg");
    let _ = fs::remove_dir_all(&parent);
    fs::create_dir_all(&pkg).unwrap();
    copy_tree(&staging, &pkg);
    let _ = fs::remove_dir_all(&staging);
    (parent, pkg)
}

fn copy_tree(src: &Path, dst: &Path) {
    fs::create_dir_all(dst).unwrap();
    for entry in fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let to = dst.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &to);
        } else {
            fs::copy(entry.path(), to).unwrap();
        }
    }
}
