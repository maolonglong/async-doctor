//! Content and metadata fingerprinting for third-party packages and loose modules.

use std::fs;
use std::path::{Path, PathBuf};

use super::short_hash;

/// Extract `(secs, subsec_nanos)` modification timestamp from filesystem metadata.
pub(crate) fn mtime_stamp(meta: &fs::Metadata) -> (u64, u32) {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| (d.as_secs(), d.subsec_nanos()))
        .unwrap_or((0, 0))
}

/// Fingerprint a wheel's `RECORD` bytes mixed with the `.dist-info` directory mtime.
pub(crate) fn fingerprint_from_record_bytes(dist_info: &Path, bytes: &[u8]) -> String {
    let mut blob = Vec::with_capacity(bytes.len() + 48);
    blob.extend_from_slice(b"RECORD:");
    blob.extend_from_slice(bytes);
    if let Ok(meta) = fs::metadata(dist_info) {
        let (secs, nanos) = mtime_stamp(&meta);
        blob.extend_from_slice(format!("|DI:{secs}.{nanos}").as_bytes());
    }
    short_hash(&blob)
}

/// Fingerprint package contents so editable/patched installs invalidate cache.
///
/// Cheap path (preferred):
/// 1. Hash `RECORD` bytes when present (covers wheel installs).
/// 2. Else hash `METADATA` / `PKG-INFO` `(len, mtime_secs, mtime_nanos)` plus dir mtime.
/// 3. Else (no dist-info): bounded `(rel, len, mtime_secs, mtime_nanos)` walk of `.py`
///    files up to depth 2 (covering both package `__init__.py` and nested subpackages).
pub fn package_content_fingerprint(dist_info_or_pkg_dir: &Path) -> String {
    let file_name = dist_info_or_pkg_dir.file_name().and_then(|n| n.to_str());
    let dist_info = if file_name.is_some_and(|n| n.ends_with(".dist-info")) {
        dist_info_or_pkg_dir.to_path_buf()
    } else {
        PathBuf::new()
    };

    if dist_info.is_dir() {
        let record = dist_info.join("RECORD");
        if record.is_file()
            && let Ok(bytes) = fs::read(&record)
        {
            return fingerprint_from_record_bytes(&dist_info, &bytes);
        }
        let meta_path = dist_info.join("METADATA");
        if let Ok(meta) = fs::metadata(&meta_path) {
            let len = meta.len();
            let (secs, nanos) = mtime_stamp(&meta);
            let di_stamp = fs::metadata(&dist_info)
                .map(|m| mtime_stamp(&m))
                .unwrap_or((0, 0));
            return short_hash(
                format!("META:{len}:{secs}.{nanos}|DI:{}.{}", di_stamp.0, di_stamp.1).as_bytes(),
            );
        }
    }

    if file_name.is_some_and(|n| n.ends_with(".egg-info")) && dist_info_or_pkg_dir.is_dir() {
        let pkg_info = dist_info_or_pkg_dir.join("PKG-INFO");
        if let Ok(meta) = fs::metadata(&pkg_info) {
            let len = meta.len();
            let (secs, nanos) = mtime_stamp(&meta);
            let ei_stamp = fs::metadata(dist_info_or_pkg_dir)
                .map(|m| mtime_stamp(&m))
                .unwrap_or((0, 0));
            return short_hash(
                format!(
                    "EGGINFO:{len}:{secs}.{nanos}|EI:{}.{}",
                    ei_stamp.0, ei_stamp.1
                )
                .as_bytes(),
            );
        }
    }

    // No dist-info: single-file module or bounded .py fingerprint.
    if dist_info_or_pkg_dir.is_file() {
        if let Ok(meta) = fs::metadata(dist_info_or_pkg_dir) {
            let len = meta.len();
            let (secs, nanos) = mtime_stamp(&meta);
            let name = file_name.unwrap_or("");
            return short_hash(format!("PYFILE:{name}:{len}:{secs}.{nanos}").as_bytes());
        }
        return short_hash(b"PYFILE:missing");
    }

    let mut entries: Vec<(String, u64, u64, u32)> = Vec::new();
    collect_py_entries_bounded(dist_info_or_pkg_dir, "", 0, 2, &mut entries);
    entries.sort();
    short_hash(format!("PY:{entries:?}").as_bytes())
}

fn collect_py_entries_bounded(
    dir: &Path,
    prefix: &str,
    depth: usize,
    max_depth: usize,
    entries: &mut Vec<(String, u64, u64, u32)>,
) {
    let Ok(walker) = fs::read_dir(dir) else {
        return;
    };
    let mut subdirs = Vec::new();
    for ent in walker.flatten() {
        let p = ent.path();
        let fname = ent.file_name();
        let Some(name) = fname.to_str() else {
            continue;
        };
        if p.extension().and_then(|e| e.to_str()) == Some("py") {
            if let Ok(meta) = ent.metadata() {
                let len = meta.len();
                let (secs, nanos) = mtime_stamp(&meta);
                let rel = if prefix.is_empty() {
                    name.to_string()
                } else {
                    format!("{prefix}/{name}")
                };
                entries.push((rel, len, secs, nanos));
            }
        } else if depth < max_depth
            && name != "__pycache__"
            && ent.file_type().is_ok_and(|ft| ft.is_dir())
        {
            subdirs.push((p, name.to_string()));
        }
    }
    subdirs.sort_by(|a, b| a.1.cmp(&b.1));
    for (sub_path, sub_name) in subdirs {
        let next_prefix = if prefix.is_empty() {
            sub_name
        } else {
            format!("{prefix}/{sub_name}")
        };
        collect_py_entries_bounded(&sub_path, &next_prefix, depth + 1, max_depth, entries);
    }
}
