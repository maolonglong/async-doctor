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
use std::sync::Mutex;

use async_doctor_core::{AnalyzeOptions, Report};
use serde::{Deserialize, Serialize};

use crate::cache::{
    CACHE_SCHEMA_VERSION, TOOL_VERSION, build_id, package_content_fingerprint, sanitize_component,
    short_hash, unique_tmp_path,
};
use crate::extract::discover_python_and_stub_files;
use crate::search_paths::discover_extra_paths;

static LAST_SCAN_IGNORE_CANDIDATES: Mutex<Option<(PathBuf, Vec<PathBuf>)>> = Mutex::new(None);

/// Take the list of `.py` files under `scan_root` that contained `"async-doctor"`
/// during the most recent directory `scan_input_fingerprint` call, if `scan_root`
/// matches.
pub fn take_ignore_candidate_files(scan_root: &Path) -> Option<Vec<PathBuf>> {
    let root_canon = scan_root
        .canonicalize()
        .unwrap_or_else(|_| scan_root.to_path_buf());
    let mut guard = LAST_SCAN_IGNORE_CANDIDATES.lock().ok()?;
    if guard.as_ref().is_some_and(|(r, _)| r == &root_canon) {
        guard.take().map(|(_, files)| files)
    } else {
        None
    }
}

pub(crate) fn clear_ignore_candidate_files() {
    if let Ok(mut guard) = LAST_SCAN_IGNORE_CANDIDATES.lock() {
        *guard = None;
    }
}

/// On-disk envelope so stale schema/tool versions never deserialize as a hit.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct ScanCacheFile {
    schema_version: u32,
    tool_version: String,
    /// Echo of the filename fingerprint (integrity check).
    input_fingerprint: String,
    report: Report,
}

#[derive(Serialize)]
struct ScanCacheFileRef<'a> {
    schema_version: u32,
    tool_version: &'a str,
    input_fingerprint: &'a str,
    report: &'a Report,
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
    let data = fs::read(path).ok()?;
    let cached: ScanCacheFile = serde_json::from_slice(&data).ok()?;
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
    let cached = ScanCacheFileRef {
        schema_version: CACHE_SCHEMA_VERSION,
        tool_version: TOOL_VERSION,
        input_fingerprint: fingerprint,
        report,
    };
    let tmp = unique_tmp_path(&path);
    let write_res = (|| -> std::io::Result<()> {
        let f = fs::File::create(&tmp)?;
        let mut writer = std::io::BufWriter::with_capacity(64 * 1024, f);
        serde_json::to_writer(&mut writer, &cached)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        writer.flush()
    })();
    if let Err(err) = write_res.and_then(|()| fs::rename(&tmp, &path)) {
        let _ = fs::remove_file(&tmp);
        return Err(err);
    }
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
    blob.push_str("scan-cache-v5\n");
    blob.push_str(&format!(
        "schema:{CACHE_SCHEMA_VERSION}\ntool:{TOOL_VERSION}\nbuild:{}\n",
        build_id()
    ));
    blob.push_str(&format!(
        "include_non_production:{}\nshow_unresolved:{}\nfollow_budget:{follow_budget}\n",
        options.include_non_production as u8, options.show_unresolved as u8,
    ));

    let root_canon = scan_root
        .canonicalize()
        .unwrap_or_else(|_| scan_root.to_path_buf());
    blob.push_str(&format!("root:{}\n", root_canon.display()));

    let fp_one = |path: &PathBuf| -> (String, String, Option<PathBuf>) {
        let rel = path
            .strip_prefix(&root_canon)
            .or_else(|_| path.strip_prefix(scan_root))
            .unwrap_or(path);
        let rel_cow = rel.to_string_lossy();
        let rel = if rel_cow.contains('\\') {
            rel_cow.replace('\\', "/")
        } else {
            rel_cow.into_owned()
        };
        let is_py = !rel.ends_with(".pyi");
        match fs::read(path) {
            Ok(bytes) => {
                let fp = short_hash(&bytes);
                let has_ignore = is_py
                    && std::str::from_utf8(&bytes).map_or(true, |s| s.contains("async-doctor"));
                (rel, fp, has_ignore.then(|| path.clone()))
            }
            Err(_) => (rel, "0".to_string(), is_py.then(|| path.clone())),
        }
    };
    let raw_files: Vec<(String, String, Option<PathBuf>)> = if first_party_files.len() >= 64 {
        let nthreads = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1)
            .min(8);
        let chunk_size = first_party_files.len().div_ceil(nthreads).max(1);
        std::thread::scope(|s| {
            let handles: Vec<_> = first_party_files
                .chunks(chunk_size)
                .map(|chunk| s.spawn(move || chunk.iter().map(fp_one).collect::<Vec<_>>()))
                .collect();
            let mut out = Vec::with_capacity(first_party_files.len());
            for h in handles {
                out.extend(h.join().expect("fingerprint worker"));
            }
            out
        })
    } else {
        first_party_files.iter().map(fp_one).collect()
    };
    let mut ignore_candidates = Vec::new();
    let mut files: Vec<(String, String)> = Vec::with_capacity(raw_files.len());
    for (rel, fp, cand) in raw_files {
        if let Some(p) = cand {
            ignore_candidates.push(p);
        }
        files.push((rel, fp));
    }
    if let Ok(mut guard) = LAST_SCAN_IGNORE_CANDIDATES.lock() {
        *guard = Some((root_canon.clone(), ignore_candidates));
    }
    files.sort();
    for (rel, fp) in &files {
        // Tag by extension so .py vs .pyi with the same stem cannot collide.
        let tag = if rel.ends_with(".pyi") { "pyi" } else { "py" };
        blob.push_str(&format!("{tag}:{rel}:{fp}\n"));
    }

    // Config files that affect search-path discovery (extraPaths / editables).
    for (depth, anc) in root_canon.ancestors().take(6).enumerate() {
        for name in [
            "pyproject.toml",
            "pyrightconfig.json",
            "pyrightconfig.toml",
            "ruff.toml",
            ".ruff.toml",
        ] {
            push_file_content(&mut blob, &format!("cfg{depth}"), &anc.join(name));
        }
    }

    let venv = resolve_venv_prefix(&root_canon);
    let site_roots = match &venv {
        Some(prefix) => {
            let canon = prefix.canonicalize().unwrap_or_else(|_| prefix.clone());
            blob.push_str(&format!("venv:{}\n", canon.display()));
            let roots = site_packages_roots_for_prefix(&canon);
            let mut pkgs = fingerprint_site_packages(&roots);
            pkgs.sort();
            for line in pkgs {
                blob.push_str(&line);
                blob.push('\n');
            }
            roots
        }
        None => {
            blob.push_str("venv:\n");
            Vec::new()
        }
    };

    // Fingerprint outside-scan-root extraPaths / editable finder targets.
    if let Ok(sys_root) = ruff_db::system::SystemPathBuf::from_path_buf(root_canon.clone()) {
        let mut existing = vec![sys_root.clone()];
        let src_dir = sys_root.join("src");
        if src_dir.as_std_path().is_dir() {
            existing.push(src_dir);
        }
        let sys_sites: Vec<ruff_db::system::SystemPathBuf> = site_roots
            .iter()
            .filter_map(|p| ruff_db::system::SystemPathBuf::from_path_buf(p.clone()).ok())
            .collect();
        let extra_roots = discover_extra_paths(&sys_root, &sys_sites, &existing);
        for (extra_idx, extra_sys) in extra_roots.iter().enumerate() {
            let extra_path = extra_sys.as_std_path();
            let extra_canon = extra_path
                .canonicalize()
                .unwrap_or_else(|_| extra_path.to_path_buf());
            if extra_canon.starts_with(&root_canon) {
                continue;
            }
            blob.push_str(&format!(
                "extra_root:{extra_idx}:{}\n",
                extra_canon.display()
            ));
            if let Ok(extra_files) = discover_python_and_stub_files(&extra_canon) {
                let mut extra_hashes = Vec::with_capacity(extra_files.len());
                for p in extra_files {
                    let rel = p
                        .strip_prefix(&extra_canon)
                        .unwrap_or(&p)
                        .to_string_lossy()
                        .replace('\\', "/");
                    let fp = content_fingerprint(&p).unwrap_or_else(|| "0".to_string());
                    extra_hashes.push((rel, fp));
                }
                extra_hashes.sort();
                for (rel, fp) in extra_hashes {
                    blob.push_str(&format!("extra_py:{extra_idx}:{rel}:{fp}\n"));
                }
            }
        }
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
            let roots = site_packages_roots_for_prefix(&canon);
            let mut pkgs = fingerprint_site_packages(&roots);
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

/// Prefer `VIRTUAL_ENV`, then ancestor `.venv`/`venv`, then `CONDA_PREFIX`.
pub(crate) fn resolve_venv_prefix(scan_root: &Path) -> Option<PathBuf> {
    if let Ok(v) = std::env::var("VIRTUAL_ENV")
        && !v.is_empty()
    {
        let p = PathBuf::from(v);
        if p.is_dir() {
            return Some(p);
        }
    }
    for base in scan_root.ancestors() {
        for name in [".venv", "venv"] {
            let p = base.join(name);
            if p.is_dir()
                && (p.join("pyvenv.cfg").is_file()
                    || p.join("lib").is_dir()
                    || p.join("Lib").is_dir())
            {
                return Some(p);
            }
        }
    }
    if let Ok(c) = std::env::var("CONDA_PREFIX")
        && !c.is_empty()
    {
        let p = PathBuf::from(c);
        if p.is_dir() {
            return Some(p);
        }
    }
    None
}

/// Enumerate sorted, deduplicated `site-packages` directories under `venv_prefix`.
pub(crate) fn site_packages_roots_for_prefix(venv_prefix: &Path) -> Vec<PathBuf> {
    let mut site_roots = Vec::new();
    for lib_dir in ["lib", "lib64"] {
        let lib = venv_prefix.join(lib_dir);
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
    }
    // Windows-style / alternate layout.
    let sp = venv_prefix.join("Lib").join("site-packages");
    if sp.is_dir() {
        site_roots.push(sp);
    }
    site_roots.sort();
    site_roots.dedup();
    site_roots
}

fn fingerprint_site_packages(site_roots: &[PathBuf]) -> Vec<String> {
    let mut out = Vec::new();
    for site in site_roots {
        let Ok(entries) = fs::read_dir(site) else {
            continue;
        };
        let mut dists: Vec<PathBuf> = Vec::new();
        let mut pths_and_finders: Vec<PathBuf> = Vec::new();
        let mut loose_py: Vec<(String, u64, u64)> = Vec::new();
        for ent in entries.flatten() {
            let p = ent.path();
            let Some(name) = p.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if name.ends_with(".dist-info") {
                dists.push(p);
            } else if name.ends_with(".pth")
                || (name.starts_with("__editable__") && name.ends_with("_finder.py"))
            {
                pths_and_finders.push(p);
            } else if name.ends_with(".py")
                && let Ok(meta) = ent.metadata()
            {
                let mtime = meta
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                loose_py.push((name.to_string(), meta.len(), mtime));
            }
        }
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
        pths_and_finders.sort();
        for pth in pths_and_finders {
            let name = pth.file_name().and_then(|n| n.to_str()).unwrap_or("?");
            let fp = content_fingerprint(&pth).unwrap_or_else(|| "0".to_string());
            out.push(format!("pth:{name}:{fp}"));
        }
        loose_py.sort();
        for (name, len, mtime) in loose_py {
            out.push(format!("loose_py:{name}:{len}:{mtime}"));
        }
    }
    out
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
