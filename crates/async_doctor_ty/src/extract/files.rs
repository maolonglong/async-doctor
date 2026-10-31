//! Orchestrate multi-pass extraction over first-party Python files.
//!
//! Warm incremental path: load per-file caches for unchanged sources whose
//! transitive import `deps_fp` still matches, run ty only on misses, merge,
//! then follow. Cold writes those caches for next time.
//!
//! `ASYNC_DOCTOR_PASS2_BATCH` controls salsa resets during pass 2:
//! - unset / `0` (default): never reset; parallel per-file salsa clones
//!   with work-stealing
//! - `locality`: reset between top-level package groups
//! - `N>0`: reset every N miss files (lower peak RSS, slower cold)

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use async_doctor_core::{CallEdge, CalleeRef, ProjectFacts};
use ruff_db::system::{SystemPath, SystemPathBuf};

use crate::cache::effective_cache_root;
use crate::db::AsyncDoctorDb;
use crate::sitepkgs::PackageMetaMemo;

use super::file_cache::orchestrate::{FileRef, extract_with_optional_cache_hits};
use super::file_cache::{
    CachedFileFacts, DepCatalogEntry, build_deps_and_fps_parallel, build_module_to_rel,
    cached_project_callees_resolvable, content_venv_stub_match, file_cache_slot_exists,
    live_deps_fp, live_deps_fps_parallel, load_file_facts_unchecked, load_project_meta,
    merge_cached_files, project_env_fingerprints, wide_miss_fallback,
};
use super::follow::follow_third_party;
use super::path::{module_name_from_relative, relative_display};

pub(crate) fn extract_files(
    project_root: &SystemPath,
    files: Vec<PathBuf>,
    cache_dir: Option<&Path>,
    follow_budget: usize,
) -> anyhow::Result<ProjectFacts> {
    let cache_root = effective_cache_root(cache_dir);
    let project_std = project_root.as_std_path();
    let (venv_fp, stub_fp) = project_env_fingerprints(project_std);

    let mut catalog: Vec<FileRef> = Vec::with_capacity(files.len());
    let mut skipped = Vec::new();
    let mut complete = true;
    for path in files {
        let system_path = match SystemPathBuf::from_path_buf(path.clone()) {
            Ok(p) => p,
            Err(_) => {
                complete = false;
                skipped.push(format!("{}: path is not valid UTF-8", path.display()));
                continue;
            }
        };
        let rel = relative_display(project_root, &system_path);
        let module = module_name_from_relative(&rel);
        catalog.push(FileRef {
            system_path,
            abs: path,
            rel,
            module,
            content_fp: None,
        });
    }
    catalog.sort_by(|a, b| a.rel.cmp(&b.rel));

    let mut cached_by_rel: HashMap<String, CachedFileFacts> = HashMap::new();
    let mut misses: Vec<usize> = Vec::new();
    let mut live_content_fps: HashMap<String, String> = HashMap::new();

    let any_slot = catalog
        .iter()
        .any(|f| file_cache_slot_exists(&cache_root, project_std, &f.rel));

    if any_slot {
        let dep_catalog: Vec<DepCatalogEntry> = catalog
            .iter()
            .map(|f| DepCatalogEntry {
                abs: f.abs.clone(),
                rel: f.rel.clone(),
                module: f.module.clone(),
            })
            .collect();
        let module_to_rel = build_module_to_rel(&dep_catalog, project_std);
        let (direct, absents_by_rel, fps) =
            build_deps_and_fps_parallel(&dep_catalog, &module_to_rel, project_std);
        live_content_fps = fps;
        for f in &mut catalog {
            if let Some(fp) = live_content_fps.get(&f.rel) {
                f.content_fp = Some(fp.clone());
            }
        }

        let slot_rels: Vec<String> = catalog
            .iter()
            .filter(|f| file_cache_slot_exists(&cache_root, project_std, &f.rel))
            .map(|f| f.rel.clone())
            .collect();
        let live_deps_by_rel =
            live_deps_fps_parallel(&slot_rels, &direct, &absents_by_rel, &live_content_fps);

        for (i, f) in catalog.iter().enumerate() {
            if !file_cache_slot_exists(&cache_root, project_std, &f.rel) {
                misses.push(i);
                continue;
            }
            let Some(cached) = load_file_facts_unchecked(&cache_root, project_std, &f.rel) else {
                misses.push(i);
                continue;
            };
            let Some(fp) = f.content_fp.as_deref() else {
                complete = false;
                skipped.push(format!("{}: cannot read for fingerprint", f.rel));
                misses.push(i);
                continue;
            };
            let live_deps = live_deps_by_rel.get(&f.rel).cloned().unwrap_or_else(|| {
                live_deps_fp(&f.rel, &direct, &absents_by_rel, &live_content_fps)
            });
            if content_venv_stub_match(&cached, fp, &venv_fp, &stub_fp)
                && !cached.deps_fp.is_empty()
                && cached.deps_fp == live_deps
            {
                cached_by_rel.insert(f.rel.clone(), cached);
            } else {
                misses.push(i);
            }
        }

        if wide_miss_fallback(misses.len(), catalog.len()) {
            cached_by_rel.clear();
            misses = (0..catalog.len()).collect();
        }
    } else {
        // Cold: skip full fingerprint until save; every file is a miss.
        misses.extend(0..catalog.len());
    }

    // All deps_fp hits: still require Project callees remappable.
    if misses.is_empty() && !catalog.is_empty() {
        let hit_list: Vec<CachedFileFacts> = catalog
            .iter()
            .filter_map(|e| cached_by_rel.get(&e.rel).cloned())
            .collect();
        let mut probe_follow = HashMap::new();
        let (probe_facts, def_map, qual_map) = merge_cached_files(&hit_list, &mut probe_follow);
        let mut remap_misses = Vec::new();
        for (i, f) in catalog.iter().enumerate() {
            if let Some(cf) = cached_by_rel.get(&f.rel)
                && !cached_project_callees_resolvable(&probe_facts, &def_map, &qual_map, cf)
            {
                remap_misses.push(i);
            }
        }
        if remap_misses.is_empty() {
            let mut facts = extract_all_cache_hits(
                project_root,
                &catalog,
                &mut cached_by_rel,
                complete,
                skipped,
                &cache_root,
                cache_dir,
                follow_budget,
                &venv_fp,
                &stub_fp,
            )?;
            facts.resolved_symbols =
                crate::twin_resolve::collect_resolved_twin_symbols(project_root.as_std_path());
            stabilize_facts(&mut facts);
            return Ok(facts);
        }
        for &i in &remap_misses {
            cached_by_rel.remove(&catalog[i].rel);
        }
        misses = remap_misses;
    }

    let mut facts = extract_with_optional_cache_hits(
        project_root,
        &catalog,
        cached_by_rel,
        &misses,
        &live_content_fps,
        &venv_fp,
        &stub_fp,
        complete,
        skipped,
        &cache_root,
        cache_dir,
        follow_budget,
    )?;
    facts.resolved_symbols =
        crate::twin_resolve::collect_resolved_twin_symbols(project_root.as_std_path());
    stabilize_facts(&mut facts);
    Ok(facts)
}

#[allow(clippy::too_many_arguments)]
fn extract_all_cache_hits(
    project_root: &SystemPath,
    catalog: &[FileRef],
    cached_by_rel: &mut HashMap<String, CachedFileFacts>,
    complete: bool,
    skipped: Vec<String>,
    cache_root: &Path,
    cache_dir: Option<&Path>,
    follow_budget: usize,
    venv_fp: &str,
    stub_fp: &str,
) -> anyhow::Result<ProjectFacts> {
    let cached: Vec<CachedFileFacts> = catalog
        .iter()
        .map(|f| cached_by_rel.remove(&f.rel).expect("all hit"))
        .collect();
    let mut follow_targets = HashMap::new();
    let (mut facts, mut def_map, mut qual_map) = merge_cached_files(&cached, &mut follow_targets);
    if let Some(meta) = load_project_meta(cache_root, project_root.as_std_path())
        && meta.venv_fingerprint == venv_fp
        && (meta.stub_fingerprint.is_empty() || meta.stub_fingerprint == stub_fp)
    {
        facts.unresolved_third_party_modules = meta.unresolved_third_party_modules;
    }
    facts.complete = complete;
    facts.skipped.extend(skipped);
    let db = AsyncDoctorDb::open(project_root, None)?;
    let followable_extra_roots: Vec<PathBuf> = db
        .followable_extra_roots()
        .iter()
        .map(|p| PathBuf::from(p.as_str()))
        .collect();
    let mut package_meta = PackageMetaMemo::new();
    follow_third_party(
        &db,
        &mut facts,
        &mut def_map,
        &mut qual_map,
        &mut follow_targets,
        &mut package_meta,
        &followable_extra_roots,
        cache_dir,
        follow_budget,
    );
    Ok(facts)
}

fn stabilize_facts(facts: &mut ProjectFacts) {
    facts
        .unresolved_imports
        .sort_by(|a, b| (&a.file, &a.module, a.line).cmp(&(&b.file, &b.module, b.line)));
    facts.unresolved_third_party_modules.sort();
    facts.unresolved_third_party_modules.dedup();

    let project_keys: Vec<String> = facts
        .functions
        .iter()
        .map(|f| format!("{}:{}", f.module, f.qualname))
        .collect();
    let callee_key = |e: &CallEdge| -> String {
        match &e.callee {
            CalleeRef::External { qualname } => format!("E:{qualname}"),
            CalleeRef::Unresolved { reason } => format!("U:{reason}"),
            CalleeRef::Offload(k) => format!("O:{}", k.as_str()),
            CalleeRef::ParamInvoke { index } => format!("I:{index}"),
            CalleeRef::Project(id) => format!(
                "P:{}",
                project_keys
                    .get(id.index())
                    .map(String::as_str)
                    .unwrap_or("")
            ),
        }
    };
    facts.edges.sort_by(|a, b| {
        (
            a.file.as_str(),
            a.line,
            a.occurrence,
            a.text.as_str(),
            callee_key(a),
            a.caller.0,
        )
            .cmp(&(
                b.file.as_str(),
                b.line,
                b.occurrence,
                b.text.as_str(),
                callee_key(b),
                b.caller.0,
            ))
    });
}
