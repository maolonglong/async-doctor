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
    CachedFileFacts, DepCatalogEntry, build_content_fps_parallel, build_deps_and_fps_parallel,
    build_module_to_rel, cached_project_callees_resolvable, content_venv_stub_match,
    file_cache_slot_exists, index_content_venv_stub_match, live_deps_fp, live_deps_fps_parallel,
    load_file_facts_unchecked, load_fp_index, merge_cached_files, project_env_fingerprints,
    wide_miss_fallback,
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
    // When true, extract may reuse hits and must persist miss slots.
    let persist_file_cache = true;

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

        // Cheap preflight: content fingerprints only. Prefer the lean
        // `_fp_index` so we never gunzip full slots just to count misses.
        let content_only = build_content_fps_parallel(&dep_catalog);
        live_content_fps = content_only;
        for f in &mut catalog {
            if let Some(fp) = live_content_fps.get(&f.rel) {
                f.content_fp = Some(fp.clone());
            }
        }
        let fp_index = load_fp_index(&cache_root, project_std);
        let mut content_misses = 0usize;
        for f in &catalog {
            let Some(fp) = f.content_fp.as_deref() else {
                content_misses += 1;
                if wide_miss_fallback(content_misses, catalog.len()) {
                    break;
                }
                continue;
            };
            let content_hit = if let Some(index) = fp_index.as_ref() {
                index
                    .entries
                    .get(&f.rel)
                    .is_some_and(|e| index_content_venv_stub_match(e, fp, &venv_fp, &stub_fp))
            } else if file_cache_slot_exists(&cache_root, project_std, &f.rel) {
                load_file_facts_unchecked(&cache_root, project_std, &f.rel)
                    .is_some_and(|c| content_venv_stub_match(&c, fp, &venv_fp, &stub_fp))
            } else {
                false
            };
            if !content_hit {
                content_misses += 1;
                if wide_miss_fallback(content_misses, catalog.len()) {
                    break;
                }
            }
        }
        if wide_miss_fallback(content_misses, catalog.len()) {
            // Still persist so the next edit can hit; skip only the *hit* path.
            misses = (0..catalog.len()).collect();
        } else {
            let module_to_rel = build_module_to_rel(&dep_catalog, project_std);
            let (direct, absents_by_rel, fps) =
                build_deps_and_fps_parallel(&dep_catalog, &module_to_rel, project_std);
            // Prefer the deps-pass fps (same hashes; keeps one source of truth).
            live_content_fps = fps;
            for f in &mut catalog {
                if let Some(fp) = live_content_fps.get(&f.rel) {
                    f.content_fp = Some(fp.clone());
                }
            }

            let classify_rels: Vec<String> = catalog.iter().map(|f| f.rel.clone()).collect();
            let live_deps_by_rel =
                live_deps_fps_parallel(&classify_rels, &direct, &absents_by_rel, &live_content_fps);

            for (i, f) in catalog.iter().enumerate() {
                let Some(fp) = f.content_fp.as_deref() else {
                    complete = false;
                    skipped.push(format!("{}: cannot read for fingerprint", f.rel));
                    misses.push(i);
                    if wide_miss_fallback(misses.len(), catalog.len()) {
                        break;
                    }
                    continue;
                };
                let live_deps = live_deps_by_rel.get(&f.rel).cloned().unwrap_or_else(|| {
                    live_deps_fp(&f.rel, &direct, &absents_by_rel, &live_content_fps)
                });
                let index_hit = fp_index.as_ref().and_then(|idx| {
                    let e = idx.entries.get(&f.rel)?;
                    if index_content_venv_stub_match(e, fp, &venv_fp, &stub_fp)
                        && !e.deps_fp.is_empty()
                        && e.deps_fp == live_deps
                    {
                        Some(())
                    } else {
                        None
                    }
                });
                if index_hit.is_some() {
                    // Load full slot only for confirmed hits.
                    if let Some(cached) =
                        load_file_facts_unchecked(&cache_root, project_std, &f.rel)
                    {
                        cached_by_rel.insert(f.rel.clone(), cached);
                    } else {
                        misses.push(i);
                    }
                } else if fp_index.is_none() {
                    // Legacy path without index: load slot to compare.
                    if !file_cache_slot_exists(&cache_root, project_std, &f.rel) {
                        misses.push(i);
                    } else if let Some(cached) =
                        load_file_facts_unchecked(&cache_root, project_std, &f.rel)
                    {
                        if content_venv_stub_match(&cached, fp, &venv_fp, &stub_fp)
                            && !cached.deps_fp.is_empty()
                            && cached.deps_fp == live_deps
                        {
                            cached_by_rel.insert(f.rel.clone(), cached);
                        } else {
                            misses.push(i);
                        }
                    } else {
                        misses.push(i);
                    }
                } else {
                    misses.push(i);
                }
                if wide_miss_fallback(misses.len(), catalog.len()) {
                    break;
                }
            }

            if wide_miss_fallback(misses.len(), catalog.len()) {
                // Hub-package invalidation: full extract, but still persist so
                // subsequent leaf edits can reuse slots.
                cached_by_rel.clear();
                misses = (0..catalog.len()).collect();
            }
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
                cache_dir,
                follow_budget,
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
        persist_file_cache,
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
    cache_dir: Option<&Path>,
    follow_budget: usize,
) -> anyhow::Result<ProjectFacts> {
    let cached: Vec<CachedFileFacts> = catalog
        .iter()
        .map(|f| cached_by_rel.remove(&f.rel).expect("all hit"))
        .collect();
    let mut follow_targets = HashMap::new();
    // Merge per-file pass-1 facts only — no global project-meta restore (v17).
    let (mut facts, mut def_map, mut qual_map) = merge_cached_files(&cached, &mut follow_targets);
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
