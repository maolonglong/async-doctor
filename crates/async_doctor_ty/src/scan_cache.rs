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

use async_doctor_core::{AnalyzeOptions, Report};
use serde::{Deserialize, Serialize};

use crate::cache::{
    BUILD_ID, CACHE_SCHEMA_VERSION, TOOL_VERSION, package_content_fingerprint, sanitize_component,
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
/// first-party `.py` / `.pyi` `(rel, content-hash)`, the active venv identity,
/// and each installed dist-info content fingerprint.
///
/// Content hashes (not whole-second mtime+size) so same-second same-size edits
/// still invalidate the whole-scan cache. Callers must pass stub paths as well
/// as `.py` sources — ty resolves against first-party `.pyi` files.
pub fn scan_input_fingerprint(
    scan_root: &Path,
    first_party_files: &[PathBuf],
    options: AnalyzeOptions,
    follow_budget: usize,
) -> String {
    let mut blob = String::with_capacity(64 * 1024);
    blob.push_str("scan-cache-v4\n");
    blob.push_str(&format!(
        "schema:{CACHE_SCHEMA_VERSION}\ntool:{TOOL_VERSION}\nbuild:{BUILD_ID}\n"
    ));
    blob.push_str(&format!(
        "include_non_production:{}\nshow_unresolved:{}\nfollow_budget:{follow_budget}\n",
        options.include_non_production as u8, options.show_unresolved as u8,
    ));

    let root_canon = scan_root
        .canonicalize()
        .unwrap_or_else(|_| scan_root.to_path_buf());
    blob.push_str(&format!("root:{}\n", root_canon.display()));

    let mut files: Vec<(String, String)> = Vec::with_capacity(first_party_files.len());
    for path in first_party_files {
        let rel = path
            .strip_prefix(&root_canon)
            .or_else(|_| path.strip_prefix(scan_root))
            .unwrap_or(path);
        let rel = rel.to_string_lossy().replace('\\', "/");
        let fp = content_fingerprint(path).unwrap_or_else(|| "0".to_string());
        files.push((rel, fp));
    }
    files.sort();
    for (rel, fp) in &files {
        // Tag by extension so .py vs .pyi with the same stem cannot collide.
        let tag = if rel.ends_with(".pyi") { "pyi" } else { "py" };
        blob.push_str(&format!("{tag}:{rel}:{fp}\n"));
    }

    // Config files that affect search-path discovery (extraPaths / editables).
    for name in [
        "pyproject.toml",
        "pyrightconfig.json",
        "pyrightconfig.toml",
        "ruff.toml",
        ".ruff.toml",
    ] {
        push_file_content(&mut blob, "cfg", &root_canon.join(name));
        if let Some(parent) = root_canon.parent() {
            push_file_content(&mut blob, "cfg", &parent.join(name));
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

fn content_fingerprint(path: &Path) -> Option<String> {
    let bytes = fs::read(path).ok()?;
    Some(short_hash(&bytes))
}

fn push_file_content(blob: &mut String, tag: &str, path: &Path) {
    if let Some(fp) = content_fingerprint(path) {
        blob.push_str(&format!(
            "{tag}:{}:{}\n",
            path.file_name().and_then(|n| n.to_str()).unwrap_or("?"),
            fp
        ));
    }
}

/// Compact fingerprint of the active venv's installed dist-info set.
///
/// Available for callers that need a standalone venv identity (the whole-scan
/// fingerprint already embeds the same dist-info lines).
#[allow(dead_code)]
pub fn venv_fingerprint(scan_root: &Path) -> String {
    let root_canon = scan_root
        .canonicalize()
        .unwrap_or_else(|_| scan_root.to_path_buf());
    let mut blob = String::from("venv-fp-v1\n");
    match resolve_venv_prefix(&root_canon) {
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

        // Same-size same-second edit must also miss (content hash, not mtime+len).
        let before = "import time\n\nasync def h():\n    time.sleep(1)\n";
        let after = "import time\n\nasync def h():\n    time.sleeq(1)\n";
        assert_eq!(before.len(), after.len());
        fs::write(&py, before).unwrap();
        let fp_a = scan_input_fingerprint(&project, &files, AnalyzeOptions::default(), 256);
        fs::write(&py, after).unwrap();
        let fp_b = scan_input_fingerprint(&project, &files, AnalyzeOptions::default(), 256);
        assert_ne!(
            fp_a, fp_b,
            "same-size edit must change content-hash fingerprint"
        );

        // First-party .pyi stubs must participate in the fingerprint.
        let stub = project.join("a.pyi");
        fs::write(&py, "async def h():\n    pass\n").unwrap();
        let fp_no_stub = scan_input_fingerprint(&project, &files, AnalyzeOptions::default(), 256);
        fs::write(&stub, "def h() -> None: ...\n").unwrap();
        let with_stub = vec![py.clone(), stub.clone()];
        let fp_stub = scan_input_fingerprint(&project, &with_stub, AnalyzeOptions::default(), 256);
        assert_ne!(fp_no_stub, fp_stub, "adding .pyi must change fingerprint");
        fs::write(&stub, "def h() -> int: ...\n").unwrap();
        let fp_stub2 = scan_input_fingerprint(&project, &with_stub, AnalyzeOptions::default(), 256);
        assert_ne!(fp_stub, fp_stub2, "editing .pyi must change fingerprint");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn stale_scan_schema_is_not_reused() {
        let dir =
            std::env::temp_dir().join(format!("async-doctor-scan-stale-{}", std::process::id()));
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
