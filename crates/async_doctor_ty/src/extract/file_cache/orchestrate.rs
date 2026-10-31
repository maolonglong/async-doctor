//! Partial-hit / cold extract with transitive import `deps_fp` invalidation.
//!
//! Content/venv/stub hits restore pass-1 from cache; a content-hit is only a
//! final hit when the live recomputed `deps_fp` matches the stored value.
//! Misses run live extract; importers of a content-miss miss automatically
//! because their transitive closure includes that file's content hash.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use async_doctor_core::{EntryLabel, FunctionFact, FunctionId, ProjectFacts};
use ruff_db::files::system_path_to_file;
use ruff_db::parsed::parsed_module;
use ruff_db::source::{line_index, source_text};
use ruff_db::system::{SystemPath, SystemPathBuf};

use crate::cache::{CACHE_SCHEMA_VERSION, TOOL_VERSION};
use crate::db::AsyncDoctorDb;
use crate::sitepkgs::{FollowTarget, PackageMetaMemo};

use crate::extract::ctx::{DefKey, TaskTargets};
use crate::extract::follow::follow_third_party;
use crate::extract::functions::{collect_functions, twin_name};

use super::deps::{
    DepCatalogEntry, build_deps_and_fps_parallel, build_module_to_rel, live_deps_fps_parallel,
};
use super::pass2_run::run_pass2_for_indices;
use super::{
    CachedFileFacts, CachedProjectMeta, MissFileSpec, append_cached_edges_only,
    cached_project_callees_resolvable, file_content_fingerprint, save_miss_file_facts_parallel,
    save_project_meta,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Pass2BatchMode {
    None,
    Fixed(usize),
    Locality,
}

pub(crate) fn pass2_batch_mode() -> Pass2BatchMode {
    match std::env::var("ASYNC_DOCTOR_PASS2_BATCH") {
        Ok(s) if s.eq_ignore_ascii_case("locality") => Pass2BatchMode::Locality,
        Ok(s) => s
            .parse::<usize>()
            .ok()
            .map(|n| {
                if n == 0 {
                    Pass2BatchMode::None
                } else {
                    Pass2BatchMode::Fixed(n)
                }
            })
            .unwrap_or(Pass2BatchMode::None),
        Err(_) => Pass2BatchMode::None,
    }
}

pub(crate) struct FileRef {
    pub(crate) system_path: SystemPathBuf,
    pub(crate) abs: PathBuf,
    pub(crate) rel: String,
    pub(crate) module: String,
    pub(crate) content_fp: Option<String>,
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn extract_with_optional_cache_hits(
    project_root: &SystemPath,
    catalog: &[FileRef],
    mut cached_by_rel: HashMap<String, CachedFileFacts>,
    initial_misses: &[usize],
    live_content_fps: &HashMap<String, String>,
    venv_fp: &str,
    stub_fp: &str,
    mut complete: bool,
    mut skipped: Vec<String>,
    cache_root: &Path,
    cache_dir: Option<&Path>,
    follow_budget: usize,
    // When false (wide-miss fallback), skip deps scrape + slot writes so the
    // run costs no more than a main cold extract.
    persist_file_cache: bool,
) -> anyhow::Result<ProjectFacts> {
    let project_std = project_root.as_std_path();
    let mut miss_set: HashSet<usize> = initial_misses.iter().copied().collect();

    let mut facts = ProjectFacts {
        complete: true,
        ..ProjectFacts::default()
    };
    let mut package_meta = PackageMetaMemo::new();
    let mut def_map: HashMap<DefKey, FunctionId> = HashMap::new();
    let mut qual_map: HashMap<(String, String), FunctionId> = HashMap::new();
    let mut property_defs: HashSet<DefKey> = HashSet::new();
    let mut class_methods: HashMap<(String, String), Vec<String>> = HashMap::new();
    let mut twins_from_cache: HashSet<FunctionId> = HashSet::new();

    let timing = std::env::var_os("ASYNC_DOCTOR_TIMING").is_some_and(|v| !v.is_empty());
    let mut db = AsyncDoctorDb::open(project_root, None)?;
    let followable_extra_roots: Vec<PathBuf> = db
        .followable_extra_roots()
        .iter()
        .map(|p| PathBuf::from(p.as_str()))
        .collect();

    // Pass 1: restore functions from content-hit caches; live collect on misses.
    let t_pass1 = std::time::Instant::now();
    {
        let program = db.program();
        for (idx, entry) in catalog.iter().enumerate() {
            if !miss_set.contains(&idx)
                && let Some(cf) = cached_by_rel.get(&entry.rel)
            {
                restore_functions_from_cache(
                    entry,
                    cf,
                    &mut facts,
                    &mut def_map,
                    &mut qual_map,
                    &mut class_methods,
                    &mut property_defs,
                    &mut twins_from_cache,
                );
                continue;
            }

            let file = match system_path_to_file(&db, &entry.system_path) {
                Ok(f) => f,
                Err(err) => {
                    complete = false;
                    skipped.push(format!("{}: {err}", entry.system_path.as_str()));
                    continue;
                }
            };
            let program_file = program.program_file(&db, file);
            let parsed = parsed_module(&db, program_file.python_file(&db)).load(&db);
            if parsed.has_invalid_syntax() {
                complete = false;
                let detail = parsed
                    .errors()
                    .first()
                    .map(|e| e.to_string())
                    .unwrap_or_else(|| "syntax error".to_string());
                skipped.push(format!("{}: {detail}", entry.rel));
                continue;
            }
            let source = source_text(&db, file);
            let index = line_index(&db, file);
            let ctx = crate::extract::ctx::FileCtx {
                db: &db,
                program_file,
                file,
                rel: entry.rel.clone(),
                module: entry.module.clone(),
                source: source.as_str().to_string(),
                index,
            };
            collect_functions(
                &ctx,
                parsed.suite(),
                &mut facts,
                &mut def_map,
                &mut qual_map,
                &mut class_methods,
                &mut property_defs,
            );
        }
    }

    for func in &mut facts.functions {
        if twins_from_cache.contains(&func.id) {
            continue;
        }
        if let Some((class, method)) = func.qualname.rsplit_once('.') {
            let key = (func.module.clone(), class.to_string());
            if let Some(methods) = class_methods.get(&key)
                && let Some(t) = twin_name(method, methods)
            {
                func.async_twins.push((method.to_string(), t));
            }
        }
    }

    // After pass-1, promote hits whose Project callees no longer remap.
    {
        let mut promoted: Vec<usize> = Vec::new();
        for (idx, entry) in catalog.iter().enumerate() {
            if miss_set.contains(&idx) {
                continue;
            }
            let Some(cf) = cached_by_rel.get(&entry.rel) else {
                continue;
            };
            if !cached_project_callees_resolvable(&facts, &def_map, &qual_map, cf) {
                promoted.push(idx);
            }
        }
        for &idx in &promoted {
            cached_by_rel.remove(&catalog[idx].rel);
            miss_set.insert(idx);
        }
    }

    if timing {
        eprintln!(
            "async-doctor file_cache: pass1={:.3}s",
            t_pass1.elapsed().as_secs_f64()
        );
    }

    let mut task_targets = TaskTargets::default();
    let mut follow_targets: HashMap<String, FollowTarget> = HashMap::new();
    seed_targets_from_cache(
        &cached_by_rel,
        &def_map,
        &mut follow_targets,
        &mut task_targets,
    );

    let mut tp_by_rel: HashMap<String, Vec<String>> = HashMap::new();
    let batch_mode = pass2_batch_mode();

    let t_pass2 = std::time::Instant::now();
    run_pass2_for_indices(
        &mut db,
        catalog,
        &miss_set,
        batch_mode,
        &mut facts,
        &mut def_map,
        &mut qual_map,
        &property_defs,
        &mut task_targets,
        &mut follow_targets,
        &mut package_meta,
        &followable_extra_roots,
        &mut tp_by_rel,
    );
    if timing {
        eprintln!(
            "async-doctor file_cache: pass2={:.3}s misses={}",
            t_pass2.elapsed().as_secs_f64(),
            miss_set.len()
        );
    }

    if !cached_by_rel.is_empty() {
        let hit_list: Vec<CachedFileFacts> = catalog
            .iter()
            .filter_map(|e| cached_by_rel.get(&e.rel).cloned())
            .collect();
        append_cached_edges_only(&mut facts, &hit_list, &def_map, &qual_map);
    }

    for &fid in &task_targets.0 {
        let f = &mut facts.functions[fid.index()];
        if f.label == EntryLabel::Other {
            f.label = EntryLabel::Task;
        }
    }

    if persist_file_cache {
        let t_deps = std::time::Instant::now();
        let dep_catalog: Vec<DepCatalogEntry> = catalog
            .iter()
            .map(|e| DepCatalogEntry {
                abs: e.abs.clone(),
                rel: e.rel.clone(),
                module: e.module.clone(),
            })
            .collect();
        let module_to_rel = build_module_to_rel(&dep_catalog, project_std);
        // One parallel pass: content fps + direct deps (avoids double-read on cold).
        let (direct, absents_by_rel, scanned_fps) =
            build_deps_and_fps_parallel(&dep_catalog, &module_to_rel, project_std);
        let content_fps = if live_content_fps.is_empty() {
            scanned_fps
        } else {
            live_content_fps.clone()
        };
        if timing {
            eprintln!(
                "async-doctor file_cache: deps_scrape={:.3}s files={}",
                t_deps.elapsed().as_secs_f64(),
                catalog.len()
            );
        }

        let mut miss_indices: Vec<usize> = miss_set.into_iter().collect();
        miss_indices.sort_unstable();
        let t_deps_fp = std::time::Instant::now();
        let miss_rels: Vec<String> = miss_indices
            .iter()
            .map(|&idx| catalog[idx].rel.clone())
            .collect();
        let deps_fps = live_deps_fps_parallel(&miss_rels, &direct, &absents_by_rel, &content_fps);
        let mut miss_specs: Vec<MissFileSpec> = Vec::with_capacity(miss_indices.len());
        for &idx in &miss_indices {
            let entry = &catalog[idx];
            let fp = entry
                .content_fp
                .clone()
                .or_else(|| content_fps.get(&entry.rel).cloned())
                .or_else(|| file_content_fingerprint(&entry.abs))
                .unwrap_or_else(|| String::from("0"));
            let deps_fp = deps_fps
                .get(&entry.rel)
                .cloned()
                .unwrap_or_else(|| String::from("0"));
            let file_tp = tp_by_rel.remove(&entry.rel).unwrap_or_default();
            miss_specs.push(MissFileSpec {
                rel: entry.rel.clone(),
                content_fp: fp,
                deps_fp,
                unresolved_tp: file_tp,
                stub_fingerprint: stub_fp.to_string(),
            });
        }
        if timing {
            eprintln!(
                "async-doctor file_cache: deps_fp_all={:.3}s",
                t_deps_fp.elapsed().as_secs_f64()
            );
        }
        let t_save = std::time::Instant::now();
        save_miss_file_facts_parallel(
            cache_root,
            project_std,
            &facts,
            &def_map,
            &follow_targets,
            &task_targets.0,
            &property_defs,
            venv_fp,
            &miss_specs,
        );
        let mut tp = facts.unresolved_third_party_modules.clone();
        tp.sort();
        tp.dedup();
        let _ = save_project_meta(
            cache_root,
            project_std,
            &CachedProjectMeta {
                schema_version: CACHE_SCHEMA_VERSION,
                tool_version: TOOL_VERSION.to_string(),
                venv_fingerprint: venv_fp.to_string(),
                stub_fingerprint: stub_fp.to_string(),
                unresolved_third_party_modules: tp,
            },
        );
        if timing {
            eprintln!(
                "async-doctor file_cache: save_slots={:.3}s misses={}",
                t_save.elapsed().as_secs_f64(),
                miss_specs.len()
            );
        }
    } else if timing {
        eprintln!(
            "async-doctor file_cache: skip_persist misses={}",
            miss_set.len()
        );
    }

    db = db.fresh_empty();
    let t_follow = std::time::Instant::now();
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
    if timing {
        eprintln!(
            "async-doctor file_cache: follow={:.3}s",
            t_follow.elapsed().as_secs_f64()
        );
    }

    facts.complete = facts.complete && complete;
    facts.skipped.extend(skipped);
    Ok(facts)
}

#[allow(clippy::too_many_arguments)]
fn restore_functions_from_cache(
    entry: &FileRef,
    cf: &CachedFileFacts,
    facts: &mut ProjectFacts,
    def_map: &mut HashMap<DefKey, FunctionId>,
    qual_map: &mut HashMap<(String, String), FunctionId>,
    class_methods: &mut HashMap<(String, String), Vec<String>>,
    property_defs: &mut HashSet<DefKey>,
    twins_from_cache: &mut HashSet<FunctionId>,
) {
    for f in &cf.functions {
        let id = FunctionId(facts.functions.len() as u32);
        def_map.insert(
            DefKey {
                file: entry.rel.clone(),
                name_start: f.name_start,
            },
            id,
        );
        qual_map.insert((f.module.clone(), f.qualname.clone()), id);
        if let Some((class, method)) = f.qualname.rsplit_once('.') {
            class_methods
                .entry((f.module.clone(), class.to_string()))
                .or_default()
                .push(method.to_string());
        }
        if !f.async_twins.is_empty() {
            twins_from_cache.insert(id);
        }
        facts.functions.push(FunctionFact {
            id,
            file: entry.rel.clone(),
            line: f.line,
            module: f.module.clone(),
            qualname: f.qualname.clone(),
            is_async: f.is_async,
            is_generator: f.is_generator,
            label: match f.label.as_str() {
                "route" => EntryLabel::Route,
                "lifecycle" => EntryLabel::Lifecycle,
                "task" => EntryLabel::Task,
                _ => EntryLabel::Other,
            },
            is_sync_route: f.is_sync_route,
            async_twins: f.async_twins.clone(),
            param_names: f.param_names.clone(),
            is_third_party: false,
            package_name: None,
            package_version: None,
            invokes_params: f.invokes_params.clone(),
            is_offload_wrapper: f.is_offload_wrapper,
        });
    }
    for &ns in &cf.property_def_name_starts {
        property_defs.insert(DefKey {
            file: entry.rel.clone(),
            name_start: ns,
        });
    }
    for u in &cf.unresolved_imports {
        facts
            .unresolved_imports
            .push(async_doctor_core::UnresolvedImport {
                file: entry.rel.clone(),
                line: u.line,
                module: u.module.clone(),
                text: u.text.clone(),
            });
    }
    facts
        .unresolved_third_party_modules
        .extend(cf.unresolved_third_party_modules.iter().cloned());
}

fn seed_targets_from_cache(
    cached_by_rel: &HashMap<String, CachedFileFacts>,
    def_map: &HashMap<DefKey, FunctionId>,
    follow_targets: &mut HashMap<String, FollowTarget>,
    task_targets: &mut TaskTargets,
) {
    for cf in cached_by_rel.values() {
        for (q, t) in &cf.follow_targets {
            follow_targets
                .entry(q.clone())
                .or_insert_with(|| FollowTarget {
                    qualname: t.qualname.clone(),
                    path: SystemPathBuf::from(t.path.as_str()),
                    name_start: t.name_start,
                    package_name: t.package_name.clone(),
                    package_version: t.package_version.clone(),
                    package_fingerprint: t.package_fingerprint.clone(),
                });
        }
        for &ns in &cf.task_target_name_starts {
            if let Some(&id) = def_map.get(&DefKey {
                file: cf.rel.clone(),
                name_start: ns,
            }) {
                task_targets.0.insert(id);
            }
        }
    }
}
