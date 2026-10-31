//! Whole-scan report cache for unchanged inputs (warm fast path).
//!
//! When first-party sources, the active venv's installed packages, analyze
//! options, and follow budget are unchanged, a warm run reloads the prior
//! [`Report`] and skips ty extract/analyze entirely.
//!
//! Layout: `{cache_root}/v{schema}/{tool}/scan/{fingerprint}.json`

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use async_doctor_core::{AnalyzeOptions, Report};
use serde::{Deserialize, Serialize};

use crate::cache::{
    CACHE_SCHEMA_VERSION, TOOL_VERSION, package_content_fingerprint, sanitize_component,
};

/// On-disk envelope so stale schema/tool versions never deserialize as a hit.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct ScanCacheFile {
    schema_version: u32,
    tool_version: String,
    /// Echo of the filename fingerprint (integrity check).
    input_fingerprint: String,
    report: Report,
}

fn scan_cache_dir(root: &Path) -> PathBuf {
    root.join(format!("v{CACHE_SCHEMA_VERSION}"))
        .join(sanitize_component(TOOL_VERSION))
        .join("scan")
}

fn scan_cache_path(root: &Path, fingerprint: &str) -> PathBuf {
    scan_cache_dir(root).join(format!("{}.json", sanitize_component(fingerprint)))
}

/// Load a cached report when present and compatible with this tool/schema.
pub fn load_scan_report(root: &Path, fingerprint: &str) -> Option<Report> {
    let path = scan_cache_path(root, fingerprint);
    let data = fs::read_to_string(path).ok()?;
    let cached: ScanCacheFile = serde_json::from_str(&data).ok()?;
    if cached.schema_version != CACHE_SCHEMA_VERSION || cached.tool_version != TOOL_VERSION {
        return None;
    }
    if cached.input_fingerprint != fingerprint {
        return None;
    }
    Some(cached.report)
}

/// Persist a scan report atomically under `fingerprint`.
pub fn save_scan_report(root: &Path, fingerprint: &str, report: &Report) -> std::io::Result<()> {
    let path = scan_cache_path(root, fingerprint);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let cached = ScanCacheFile {
        schema_version: CACHE_SCHEMA_VERSION,
        tool_version: TOOL_VERSION.to_string(),
        input_fingerprint: fingerprint.to_string(),
        report: report.clone(),
    };
    let data = serde_json::to_string(&cached)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    let tmp = path.with_extension(format!("json.tmp.{}", std::process::id()));
    {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(data.as_bytes())?;
        f.sync_all()?;
    }
    fs::rename(&tmp, &path)?;
    Ok(())
}

/// Fingerprint everything that can change graded output for a scan root.
///
/// Includes schema/tool versions, analyze options, follow budget, every
/// first-party `.py` `(rel, len, mtime_secs)`, the active venv identity, and
/// each installed dist-info content fingerprint.
pub fn scan_input_fingerprint(
    scan_root: &Path,
    first_party_files: &[PathBuf],
    options: AnalyzeOptions,
    follow_budget: usize,
) -> String {
    let mut blob = String::with_capacity(64 * 1024);
    blob.push_str("scan-cache-v1\n");
    blob.push_str(&format!(
        "schema:{CACHE_SCHEMA_VERSION}\ntool:{TOOL_VERSION}\n"
    ));
    blob.push_str(&format!(
        "include_non_production:{}\nshow_unresolved:{}\nfollow_budget:{follow_budget}\n",
        options.include_non_production as u8, options.show_unresolved as u8,
    ));

    let root_canon = scan_root.canonicalize().unwrap_or_else(|_| scan_root.to_path_buf());
    blob.push_str(&format!("root:{}\n", root_canon.display()));

    let mut files: Vec<(String, u64, u64)> = Vec::with_capacity(first_party_files.len());
    for path in first_party_files {
        let rel = path
            .strip_prefix(&root_canon)
            .or_else(|_| path.strip_prefix(scan_root))
            .unwrap_or(path);
        let rel = rel.to_string_lossy().replace('\\', "/");
        let (len, mtime) = match fs::metadata(path) {
            Ok(meta) => (meta.len(), mtime_secs(&meta)),
            Err(_) => (0, 0),
        };
        files.push((rel, len, mtime));
    }
    files.sort();
    for (rel, len, mtime) in &files {
        blob.push_str(&format!("py:{rel}:{len}:{mtime}\n"));
    }

    // Config files that affect search-path discovery (extraPaths / editables).
    for name in [
        "pyproject.toml",
        "pyrightconfig.json",
        "pyrightconfig.toml",
        "ruff.toml",
        ".ruff.toml",
    ] {
        push_file_meta(&mut blob, "cfg", &root_canon.join(name));
        if let Some(parent) = root_canon.parent() {
            push_file_meta(&mut blob, "cfg", &parent.join(name));
        }
    }

    let venv = resolve_venv_prefix(&root_canon);
    match &venv {
        Some(prefix) => {
            let canon = prefix.canonicalize().unwrap_or_else(|_| prefix.clone());
            blob.push_str(&format!("venv:{}\n", canon.display()));
            let mut pkgs = fingerprint_site_packages(&canon);
            pkgs.sort();
            for line in pkgs {
                blob.push_str(&line);
                blob.push('\n');
            }
        }
        None => blob.push_str("venv:\n"),
    }

    short_hash(blob.as_bytes())
}

fn push_file_meta(blob: &mut String, tag: &str, path: &Path) {
    if let Ok(meta) = fs::metadata(path)
        && meta.is_file()
    {
        blob.push_str(&format!(
            "{tag}:{}:{}:{}\n",
            path.file_name().and_then(|n| n.to_str()).unwrap_or("?"),
            meta.len(),
            mtime_secs(&meta)
        ));
    }
}

fn mtime_secs(meta: &fs::Metadata) -> u64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Prefer `VIRTUAL_ENV`, then in-project / parent `.venv`/`venv`.
fn resolve_venv_prefix(scan_root: &Path) -> Option<PathBuf> {
    if let Ok(v) = std::env::var("VIRTUAL_ENV")
        && !v.is_empty()
    {
        let p = PathBuf::from(v);
        if p.is_dir() {
            return Some(p);
        }
    }
    for base in [scan_root, scan_root.parent().unwrap_or(scan_root)] {
        for name in [".venv", "venv"] {
            let p = base.join(name);
            if p.is_dir() {
                return Some(p);
            }
        }
    }
    None
}

fn fingerprint_site_packages(venv_prefix: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut site_roots = Vec::new();
    let lib = venv_prefix.join("lib");
    if let Ok(entries) = fs::read_dir(&lib) {
        for ent in entries.flatten() {
            let name = ent.file_name();
            let name = name.to_string_lossy();
            if name.starts_with("python") {
                let sp = ent.path().join("site-packages");
                if sp.is_dir() {
                    site_roots.push(sp);
                }
            }
        }
    }
    // Windows-style / alternate layout.
    let sp = venv_prefix.join("Lib").join("site-packages");
    if sp.is_dir() {
        site_roots.push(sp);
    }
    site_roots.sort();
    site_roots.dedup();

    for site in site_roots {
        let Ok(entries) = fs::read_dir(&site) else {
            continue;
        };
        let mut dists: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.ends_with(".dist-info"))
            })
            .collect();
        dists.sort();
        for dist in dists {
            let name = dist
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("?")
                .to_string();
            let fp = package_content_fingerprint(&dist);
            out.push(format!("pkg:{name}:{fp}"));
        }
    }
    out
}

fn short_hash(bytes: &[u8]) -> String {
    let mut hash: u64 = 0xcbf29ce484222325;
    for b in bytes {
        hash ^= u64::from(*b);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_doctor_core::Report;

    #[test]
    fn scan_cache_roundtrip_and_fingerprint_stable() {
        let dir = std::env::temp_dir().join(format!(
            "async-doctor-scan-cache-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let root = dir.join("async-doctor");
        let project = dir.join("proj");
        fs::create_dir_all(&project).unwrap();
        let py = project.join("a.py");
        fs::write(&py, "async def h():\n    pass\n").unwrap();

        let files = vec![py.clone()];
        let fp1 = scan_input_fingerprint(&project, &files, AnalyzeOptions::default(), 256);
        let fp2 = scan_input_fingerprint(&project, &files, AnalyzeOptions::default(), 256);
        assert_eq!(fp1, fp2);

        let report = Report::empty_complete();
        save_scan_report(&root, &fp1, &report).unwrap();
        let loaded = load_scan_report(&root, &fp1).unwrap();
        assert_eq!(loaded, report);

        // Budget change must miss.
        let fp_budget = scan_input_fingerprint(&project, &files, AnalyzeOptions::default(), 1);
        assert_ne!(fp1, fp_budget);
        assert!(load_scan_report(&root, &fp_budget).is_none());

        // Content change must miss.
        fs::write(&py, "async def h():\n    await x()\n").unwrap();
        let fp3 = scan_input_fingerprint(&project, &files, AnalyzeOptions::default(), 256);
        assert_ne!(fp1, fp3);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn stale_scan_schema_is_not_reused() {
        let dir = std::env::temp_dir().join(format!(
            "async-doctor-scan-stale-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        let root = dir.join("async-doctor");
        let report = Report::empty_complete();
        save_scan_report(&root, "deadbeefdeadbeef", &report).unwrap();
        let path = scan_cache_path(&root, "deadbeefdeadbeef");
        let mut spoofed: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        spoofed["schema_version"] = serde_json::json!(CACHE_SCHEMA_VERSION - 1);
        fs::write(&path, serde_json::to_string(&spoofed).unwrap()).unwrap();
        assert!(load_scan_report(&root, "deadbeefdeadbeef").is_none());
        let _ = fs::remove_dir_all(&dir);
    }
}
