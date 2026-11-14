//! Whole-scan report cache for unchanged inputs (warm fast path).
//!
//! When first-party sources, the active venv's installed packages, analyze
//! options, and follow budget are unchanged, a warm run reloads the prior
//! [`Report`] and skips ty extract/analyze entirely.
//!
//! Layout: `{cache_root}/v{schema}/{tool}/scan/{root_prefix}-{input_hash}.json`
//! At most `MAX_SCAN_ENTRIES_PER_ROOT = 3` most-recent entries are retained per
//! canonical `scan_root`, and sibling `v{old}` schema directories older than
//! [`CACHE_SCHEMA_VERSION`] are opportunistically pruned on write.

use std::collections::HashSet;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use async_doctor_core::{AnalyzeOptions, Report};
use serde::{Deserialize, Serialize};

use crate::cache::{
    CACHE_SCHEMA_VERSION, TOOL_VERSION, build_id, mtime_stamp, package_content_fingerprint,
    prune_stale_schema_dirs, sanitize_component, short_hash, unique_tmp_path,
};
use crate::extract::discover_python_and_stub_files;
use crate::search_paths::discover_extra_paths;

const MAX_SCAN_ENTRIES_PER_ROOT: usize = 3;

/// Whole-scan cache key plus the `.py` files under `scan_root` that contained
/// `"async-doctor"` during fingerprinting (used to scope inline-suppression parsing
/// without any process-global state).
#[derive(Debug, Clone)]
pub(crate) struct ScanFingerprint {
    pub key: String,
    pub ignore_candidate_files: Vec<PathBuf>,
}

/// On-disk envelope so stale schema/tool versions never deserialize as a hit.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct ScanCacheFile {
    schema_version: u32,
    tool_version: String,
    /// Echo of the filename fingerprint (integrity check).
    input_fingerprint: String,
    report: Report,
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    item_spans: std::collections::BTreeMap<String, Vec<(String, u32, u32)>>,
}

#[derive(Serialize)]
struct ScanCacheFileRef<'a> {
    schema_version: u32,
    tool_version: &'a str,
    input_fingerprint: &'a str,
    report: &'a Report,
    #[serde(skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    item_spans: &'a std::collections::BTreeMap<String, Vec<(String, u32, u32)>>,
}

fn scan_cache_dir(root: &Path) -> PathBuf {
    root.join(format!("v{CACHE_SCHEMA_VERSION}"))
        .join(sanitize_component(TOOL_VERSION))
        .join("scan")
}

fn scan_cache_path(root: &Path, fingerprint: &str) -> PathBuf {
    scan_cache_dir(root).join(format!("{}.json", sanitize_component(fingerprint)))
}

fn fingerprint_root_prefix(sanitized_fp: &str) -> Option<&str> {
    sanitized_fp
        .split_once('-')
        .map(|(prefix, _)| prefix)
        .filter(|p| !p.is_empty())
}

/// Reconcile `.{prefix}.order` with `.json` files on disk, promote `stem` to
/// most-recent, and evict oldest entries beyond `MAX_SCAN_ENTRIES_PER_ROOT`.
fn touch_and_gc_root_entries(scan_dir: &Path, prefix: &str, stem: &str, evict: bool) {
    let prefix_dash = format!("{prefix}-");
    let mut disk_stems: Vec<((u64, u32), String)> = Vec::new();
    let Ok(entries) = fs::read_dir(scan_dir) else {
        return;
    };
    for ent in entries.flatten() {
        let fname = ent.file_name();
        let Some(name) = fname.to_str() else {
            continue;
        };
        let Some(file_stem) = name.strip_suffix(".json") else {
            continue;
        };
        if !file_stem.starts_with(&prefix_dash) {
            continue;
        }
        let stamp = ent.metadata().map(|m| mtime_stamp(&m)).unwrap_or((0, 0));
        disk_stems.push((stamp, file_stem.to_string()));
    }
    let on_disk: HashSet<String> = disk_stems.iter().map(|(_, s)| s.clone()).collect();
    let order_path = scan_dir.join(format!(".{prefix}.order"));
    let existing_text = fs::read_to_string(&order_path).unwrap_or_default();
    if !evict
        && existing_text
            .lines()
            .next_back()
            .is_some_and(|last| last == stem)
        && on_disk.len() <= MAX_SCAN_ENTRIES_PER_ROOT
    {
        return;
    }

    let mut ordered: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for line in existing_text.lines() {
        let s = line.trim();
        if !s.is_empty() && on_disk.contains(s) && seen.insert(s.to_string()) {
            ordered.push(s.to_string());
        }
    }
    disk_stems.sort();
    for (_, s) in disk_stems {
        if s != stem && seen.insert(s.clone()) {
            ordered.push(s);
        }
    }
    ordered.retain(|s| s != stem);
    if on_disk.contains(stem) {
        ordered.push(stem.to_string());
    }
    if evict {
        while ordered.len() > MAX_SCAN_ENTRIES_PER_ROOT {
            let victim = ordered.remove(0);
            let _ = fs::remove_file(scan_dir.join(format!("{victim}.json")));
        }
    }
    let tmp = unique_tmp_path(&order_path);
    if fs::write(&tmp, ordered.join("\n"))
        .and_then(|()| fs::rename(&tmp, &order_path))
        .is_err()
    {
        let _ = fs::remove_file(&tmp);
    }
}

/// Load a cached report when present and compatible with this tool/schema.
pub fn load_scan_report(root: &Path, fingerprint: &str) -> Option<Report> {
    let path = scan_cache_path(root, fingerprint);
    let data = fs::read(&path).ok()?;
    let cached: ScanCacheFile = serde_json::from_slice(&data).ok()?;
    if cached.schema_version != CACHE_SCHEMA_VERSION || cached.tool_version != TOOL_VERSION {
        return None;
    }
    if cached.input_fingerprint != fingerprint {
        return None;
    }
    let sanitized = sanitize_component(fingerprint);
    if let (Some(scan_dir), Some(prefix)) = (path.parent(), fingerprint_root_prefix(&sanitized)) {
        touch_and_gc_root_entries(scan_dir, prefix, &sanitized, false);
    }
    let mut report = cached.report;
    report.item_spans = cached.item_spans;
    Some(report)
}

/// Persist a scan report atomically under `fingerprint`, pruning stale schema
/// directories and capping per-root scan cache files to `K = 3`.
pub fn save_scan_report(root: &Path, fingerprint: &str, report: &Report) -> std::io::Result<()> {
    prune_stale_schema_dirs(root);
    let path = scan_cache_path(root, fingerprint);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let cached = ScanCacheFileRef {
        schema_version: CACHE_SCHEMA_VERSION,
        tool_version: TOOL_VERSION,
        input_fingerprint: fingerprint,
        report,
        item_spans: &report.item_spans,
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
    let sanitized = sanitize_component(fingerprint);
    if let (Some(scan_dir), Some(prefix)) = (path.parent(), fingerprint_root_prefix(&sanitized)) {
        touch_and_gc_root_entries(scan_dir, prefix, &sanitized, true);
    }
    Ok(())
}

/// Fingerprint everything that can change graded output for a scan root.
#[cfg(test)]
pub fn scan_input_fingerprint(
    scan_root: &Path,
    first_party_files: &[PathBuf],
    options: AnalyzeOptions,
    follow_budget: usize,
) -> String {
    scan_input_fingerprint_with_python(scan_root, first_party_files, options, follow_budget, None)
        .key
}

pub(crate) fn scan_input_fingerprint_with_python(
    scan_root: &Path,
    first_party_files: &[PathBuf],
    options: AnalyzeOptions,
    follow_budget: usize,
    python: Option<&Path>,
) -> ScanFingerprint {
    let mut blob = String::with_capacity(64 * 1024);
    blob.push_str("scan-cache-v8\n");
    blob.push_str(&format!(
        "schema:{CACHE_SCHEMA_VERSION}\ntool:{TOOL_VERSION}\nbuild:{}\n",
        build_id()
    ));
    blob.push_str(&format!(
        "include_non_production:{}\nshow_unresolved:{}\nfollow_budget:{follow_budget}\n",
        options.include_non_production as u8, options.show_unresolved as u8,
    ));
    match python {
        Some(py) => {
            let py_canon = py.canonicalize().unwrap_or_else(|_| py.to_path_buf());
            blob.push_str(&format!("python_arg:{}\n", py_canon.display()));
        }
        None => blob.push_str("python_arg:\n"),
    }

    let root_canon = scan_root
        .canonicalize()
        .unwrap_or_else(|_| scan_root.to_path_buf());
    let root_prefix = short_hash(format!("scan-root-v1:{}", root_canon.display()).as_bytes());
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
    ignore_candidates.sort();
    files.sort();
    for (rel, fp) in &files {
        let tag = if rel.ends_with(".pyi") { "pyi" } else { "py" };
        blob.push_str(&format!("{tag}:{rel}:{fp}\n"));
    }

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

    let (env_kind, env_prefix, site_roots) =
        crate::env_discovery::discover_site_packages_for_path(&root_canon, python);
    match &env_prefix {
        Some(prefix) => {
            let canon = prefix.canonicalize().unwrap_or_else(|_| prefix.clone());
            blob.push_str(&format!("venv:{env_kind:?}:{}\n", canon.display()));
        }
        None => blob.push_str("venv:\n"),
    }
    if !site_roots.is_empty() {
        let mut pkgs = fingerprint_site_packages(&site_roots);
        pkgs.sort();
        for line in pkgs {
            blob.push_str(&line);
            blob.push('\n');
        }
    }

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

    ScanFingerprint {
        key: format!("{root_prefix}-{}", short_hash(blob.as_bytes())),
        ignore_candidate_files: ignore_candidates,
    }
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
#[allow(dead_code)]
pub fn venv_fingerprint(scan_root: &Path) -> String {
    let root_canon = scan_root
        .canonicalize()
        .unwrap_or_else(|_| scan_root.to_path_buf());
    let mut blob = String::from("venv-fp-v2\n");
    let (env_kind, env_prefix, roots) =
        crate::env_discovery::discover_site_packages_for_path(&root_canon, None);
    match env_prefix {
        Some(prefix) => {
            let canon = prefix.canonicalize().unwrap_or_else(|_| prefix.clone());
            blob.push_str(&format!("venv:{env_kind:?}:{}\n", canon.display()));
        }
        None => blob.push_str("venv:\n"),
    }
    if !roots.is_empty() {
        let mut pkgs = fingerprint_site_packages(&roots);
        pkgs.sort();
        for line in pkgs {
            blob.push_str(&line);
            blob.push('\n');
        }
    }
    short_hash(blob.as_bytes())
}

fn fingerprint_site_packages(site_roots: &[PathBuf]) -> Vec<String> {
    let mut out = Vec::new();
    for site in site_roots {
        let Ok(entries) = fs::read_dir(site) else {
            continue;
        };
        let mut dists: Vec<PathBuf> = Vec::new();
        let mut pths_and_finders: Vec<PathBuf> = Vec::new();
        let mut loose_py: Vec<(String, u64, u64, u32)> = Vec::new();
        for ent in entries.flatten() {
            let p = ent.path();
            let Some(name) = p.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if name.ends_with(".dist-info") || name.ends_with(".egg-info") {
                dists.push(p);
            } else if name.ends_with(".pth")
                || (name.starts_with("__editable__") && name.ends_with("_finder.py"))
            {
                pths_and_finders.push(p);
            } else if name.ends_with(".py")
                && let Ok(meta) = ent.metadata()
            {
                let (secs, nanos) = mtime_stamp(&meta);
                loose_py.push((name.to_string(), meta.len(), secs, nanos));
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
        for (name, len, secs, nanos) in loose_py {
            out.push(format!("loose_py:{name}:{len}:{secs}.{nanos}"));
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
        assert!(
            fp1.contains('-'),
            "fingerprint should carry root prefix: {fp1}"
        );

        let report = Report::empty_complete();
        save_scan_report(&root, &fp1, &report).unwrap();
        let loaded = load_scan_report(&root, &fp1).unwrap();
        assert_eq!(loaded, report);

        let fp_budget = scan_input_fingerprint(&project, &files, AnalyzeOptions::default(), 1);
        assert_ne!(fp1, fp_budget);
        assert!(load_scan_report(&root, &fp_budget).is_none());

        fs::write(&py, "async def h():\n    await x()\n").unwrap();
        let fp3 = scan_input_fingerprint(&project, &files, AnalyzeOptions::default(), 256);
        assert_ne!(fp1, fp3);

        let before = "import time\n\nasync def h():\n    time.sleep(1)\n";
        let after = "import time\n\nasync def h():\n    time.sleeq(1)\n";
        assert_eq!(before.len(), after.len());
        fs::write(&py, before).unwrap();
        let fp_a = scan_input_fingerprint(&project, &files, AnalyzeOptions::default(), 256);
        fs::write(&py, after).unwrap();
        let fp_b = scan_input_fingerprint(&project, &files, AnalyzeOptions::default(), 256);
        assert_ne!(fp_a, fp_b);

        let stub = project.join("a.pyi");
        fs::write(&py, "async def h():\n    pass\n").unwrap();
        let fp_no_stub = scan_input_fingerprint(&project, &files, AnalyzeOptions::default(), 256);
        fs::write(&stub, "def h() -> None: ...\n").unwrap();
        let with_stub = vec![py.clone(), stub.clone()];
        let fp_stub = scan_input_fingerprint(&project, &with_stub, AnalyzeOptions::default(), 256);
        assert_ne!(fp_no_stub, fp_stub);
        fs::write(&stub, "def h() -> int: ...\n").unwrap();
        let fp_stub2 = scan_input_fingerprint(&project, &with_stub, AnalyzeOptions::default(), 256);
        assert_ne!(fp_stub, fp_stub2);

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
