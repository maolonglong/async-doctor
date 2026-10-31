//! Orchestrate multi-pass extraction over first-party Python files.
//!
//! Warm incremental path: load per-file caches for unchanged sources, run ty
//! only on misses, merge, then follow. Cold writes those caches for next time.
//!
//! `ASYNC_DOCTOR_PASS2_BATCH` controls salsa resets during pass 2:
//! - unset / `0` (default): never reset; parallel per-file salsa clones
//! - `locality`: reset between top-level package groups
//! - `N>0`: reset every N miss files

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use async_doctor_core::{EntryLabel, FunctionFact, FunctionId, ProjectFacts};
use ruff_db::files::system_path_to_file;
use ruff_db::parsed::parsed_module;
use ruff_db::source::{line_index, source_text};
use ruff_db::system::{SystemPath, SystemPathBuf};
use ruff_python_ast::visitor::Visitor;
use ty_python_semantic::SemanticModel;

use crate::cache::effective_cache_root;
use crate::db::AsyncDoctorDb;
use crate::scan_cache::venv_fingerprint;
use crate::sitepkgs::{FollowTarget, PackageMetaMemo};

use super::collector::CallCollector;
use super::constants::collect_module_constants;
use super::ctx::{DefKey, FileCtx, TaskTargets};
use super::facts_dump::dump_facts_pre_follow;
use super::facts_merge::{append_cached_edges_only, stabilize_facts};
use super::file_cache::{
    CachedFileFacts, CachedProjectMeta, MissFileSpec, direct_import_deps,
    expand_transitive_import_fps, file_cache_slot_exists, file_content_fingerprint,
    import_fps_for_source, import_fps_still_valid, insert_module_fp_aliases,
    load_file_facts_unchecked, load_project_meta, merge_cached_files,
    save_miss_file_facts_parallel, save_project_meta,
};
use super::follow::follow_third_party;
use super::functions::{collect_functions, twin_name};
use super::pass2::{Pass2FileSpec, Pass2Shared, merge_pass2_results, run_pass2_parallel};
use super::path::{module_name_from_relative, relative_display};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pass2BatchMode {
    None,
    Fixed(usize),
    Locality,
}

fn pass2_batch_mode() -> Pass2BatchMode {
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

struct FileRef {
    system_path: SystemPathBuf,
    abs: PathBuf,
    rel: String,
    module: String,
    content_fp: Option<String>,
}

pub(crate) fn extract_files(
    project_root: &SystemPath,
    files: Vec<PathBuf>,
    cache_dir: Option<&Path>,
    follow_budget: usize,
) -> anyhow::Result<ProjectFacts> {
    let cache_root = effective_cache_root(cache_dir);
    let project_std = project_root.as_std_path();
    let venv_fp = venv_fingerprint(project_std);

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

    // Fingerprint only when a cache slot exists (warm/partial). Cold skips the
    // full-tree hash pass; module_fps for saves is filled lazily below.
    let mut module_fps: BTreeMap<String, String> = BTreeMap::new();
    let mut cached_by_rel: HashMap<String, CachedFileFacts> = HashMap::new();
    let mut misses: Vec<usize> = Vec::new();
    let any_slot = catalog
        .iter()
        .any(|f| file_cache_slot_exists(&cache_root, project_std, &f.rel));
    if any_slot {
        for f in &mut catalog {
            if let Some(fp) = file_content_fingerprint(&f.abs) {
                insert_module_fp_aliases(&mut module_fps, project_std, &f.module, fp.clone());
                f.content_fp = Some(fp);
            }
        }
    }
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
        let hit = cached.content_fingerprint == fp
            && cached.venv_fingerprint == venv_fp
            && import_fps_still_valid(&cached.import_fps, &module_fps);
        if hit {
            cached_by_rel.insert(f.rel.clone(), cached);
        } else {
            misses.push(i);
        }
    }

    if misses.is_empty() && !catalog.is_empty() {
        return extract_all_cache_hits(
            project_root,
            &catalog,
            &mut cached_by_rel,
            complete,
            skipped,
            &cache_root,
            cache_dir,
            follow_budget,
            &venv_fp,
        );
    }

    extract_with_optional_cache_hits(
        project_root,
        &catalog,
        &cached_by_rel,
        &misses,
        &mut module_fps,
        &venv_fp,
        complete,
        skipped,
        &cache_root,
        cache_dir,
        follow_budget,
    )
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
) -> anyhow::Result<ProjectFacts> {
    let cached: Vec<CachedFileFacts> = catalog
        .iter()
        .map(|f| cached_by_rel.remove(&f.rel).expect("all hit"))
        .collect();
    let mut follow_targets = HashMap::new();
    let (mut facts, mut def_map, mut qual_map) = merge_cached_files(&cached, &mut follow_targets);
    if let Some(meta) = load_project_meta(cache_root, project_root.as_std_path())
        && meta.venv_fingerprint == venv_fp
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
    dump_facts_pre_follow("all-hit", &facts, &follow_targets);
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
    dump_facts_pre_follow("all-hit-post-follow", &facts, &follow_targets);
    stabilize_facts(&mut facts);
    Ok(facts)
}

#[allow(clippy::too_many_arguments)]
fn extract_with_optional_cache_hits(
    project_root: &SystemPath,
    catalog: &[FileRef],
    cached_by_rel: &HashMap<String, CachedFileFacts>,
    misses: &[usize],
    module_fps: &mut BTreeMap<String, String>,
    venv_fp: &str,
    mut complete: bool,
    mut skipped: Vec<String>,
    cache_root: &Path,
    cache_dir: Option<&Path>,
    follow_budget: usize,
) -> anyhow::Result<ProjectFacts> {
    let miss_set: HashSet<usize> = misses.iter().copied().collect();
    let mut facts = ProjectFacts {
        complete: true,
        ..ProjectFacts::default()
    };
    let mut package_meta = PackageMetaMemo::new();
    let mut def_map: HashMap<DefKey, FunctionId> = HashMap::new();
    let mut qual_map: HashMap<(String, String), FunctionId> = HashMap::new();
    let mut property_defs: HashSet<DefKey> = HashSet::new();
    let mut class_methods: HashMap<(String, String), Vec<String>> = HashMap::new();
    // Functions restored from cache already carry async_twins — skip re-derive.
    let mut twins_from_cache: HashSet<FunctionId> = HashSet::new();

    let mut db = AsyncDoctorDb::open(project_root, None)?;
    let followable_extra_roots: Vec<PathBuf> = db
        .followable_extra_roots()
        .iter()
        .map(|p| PathBuf::from(p.as_str()))
        .collect();

    // Pass 1: functions from cache hits or live collect on misses.
    {
        let program = db.program();
        for (idx, entry) in catalog.iter().enumerate() {
            if !miss_set.contains(&idx)
                && let Some(cf) = cached_by_rel.get(&entry.rel)
            {
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
            let ctx = FileCtx {
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

    let mut task_targets = TaskTargets::default();
    let mut follow_targets: HashMap<String, FollowTarget> = HashMap::new();
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

    let miss_entries: Vec<&FileRef> = misses.iter().map(|&i| &catalog[i]).collect();
    let mut tp_by_rel: HashMap<String, Vec<String>> = HashMap::new();
    let batch_mode = pass2_batch_mode();
    if matches!(batch_mode, Pass2BatchMode::None) && !miss_entries.is_empty() {
        let base_fn_len = facts.functions.len();
        let shared = Pass2Shared {
            functions: std::sync::Arc::new(facts.functions.clone()),
            def_map: std::sync::Arc::new(def_map.clone()),
            qual_map: std::sync::Arc::new(qual_map.clone()),
            property_defs: std::sync::Arc::new(property_defs.clone()),
            followable_extra_roots: std::sync::Arc::new(followable_extra_roots.clone()),
        };
        let specs: Vec<Pass2FileSpec> = miss_entries
            .iter()
            .map(|e| Pass2FileSpec {
                system_path: e.system_path.clone(),
                rel: e.rel.clone(),
                module: e.module.clone(),
            })
            .collect();
        let results = run_pass2_parallel(&db, &specs, &shared);
        for r in &results {
            tp_by_rel.insert(r.rel.clone(), r.unresolved_third_party_modules.clone());
        }
        merge_pass2_results(
            &mut facts,
            &mut def_map,
            &mut qual_map,
            &mut follow_targets,
            &mut task_targets,
            base_fn_len,
            results,
        );
    } else {
        run_pass2_batched(
            &mut db,
            &miss_entries,
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
    }

    if !cached_by_rel.is_empty() {
        let hit_list: Vec<CachedFileFacts> = catalog
            .iter()
            .filter(|e| cached_by_rel.contains_key(&e.rel))
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

    let project_std = project_root.as_std_path();
    let canon_modules: HashSet<String> = catalog.iter().map(|e| e.module.clone()).collect();

    // Ensure module_fps covers the whole catalog (with package-root aliases)
    // before encoding import keys.
    for entry in catalog {
        if module_fps.contains_key(&entry.module) {
            continue;
        }
        let fp = file_content_fingerprint(&entry.abs).unwrap_or_else(|| String::from("0"));
        insert_module_fp_aliases(module_fps, project_std, &entry.module, fp);
    }

    // Direct first-party import graph for transitive invalidation (canonical keys).
    let mut direct_deps: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for entry in catalog {
        let source = std::fs::read_to_string(&entry.abs).unwrap_or_default();
        let deps = direct_import_deps(
            &source,
            &entry.module,
            module_fps,
            &canon_modules,
            project_std,
        );
        direct_deps.insert(entry.module.clone(), deps);
    }

    // Persist first-party per-file caches *before* follow (parallel encode/save).
    let mut miss_specs: Vec<MissFileSpec> = Vec::with_capacity(misses.len());
    for &idx in misses {
        let entry = &catalog[idx];
        let fp = entry.content_fp.clone().unwrap_or_else(|| {
            module_fps
                .get(&entry.module)
                .cloned()
                .or_else(|| file_content_fingerprint(&entry.abs))
                .unwrap_or_else(|| String::from("0"))
        });
        let source = std::fs::read_to_string(&entry.abs).unwrap_or_default();
        let direct = import_fps_for_source(
            &source,
            &entry.module,
            module_fps,
            &canon_modules,
            project_std,
        );
        let import_fps = expand_transitive_import_fps(&direct, &direct_deps, module_fps);
        let file_tp = tp_by_rel.remove(&entry.rel).unwrap_or_default();
        miss_specs.push((entry.rel.clone(), fp, import_fps, file_tp));
    }
    save_miss_file_facts_parallel(
        cache_root,
        project_root.as_std_path(),
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
        project_root.as_std_path(),
        &CachedProjectMeta {
            schema_version: crate::cache::CACHE_SCHEMA_VERSION,
            tool_version: crate::cache::TOOL_VERSION.to_string(),
            venv_fingerprint: venv_fp.to_string(),
            unresolved_third_party_modules: tp,
        },
    );

    dump_facts_pre_follow("cold-or-partial", &facts, &follow_targets);

    // Drop first-party SemanticModel memos before follow so cold matches all-hit.
    db = db.fresh_empty();

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
    dump_facts_pre_follow("cold-post-follow", &facts, &follow_targets);

    facts.complete = facts.complete && complete;
    facts.skipped.extend(skipped);
    stabilize_facts(&mut facts);

    Ok(facts)
}

#[allow(clippy::too_many_arguments)]
fn run_pass2_batched(
    db: &mut AsyncDoctorDb,
    miss_entries: &[&FileRef],
    batch_mode: Pass2BatchMode,
    facts: &mut ProjectFacts,
    def_map: &mut HashMap<DefKey, FunctionId>,
    qual_map: &mut HashMap<(String, String), FunctionId>,
    property_defs: &HashSet<DefKey>,
    task_targets: &mut TaskTargets,
    follow_targets: &mut HashMap<String, FollowTarget>,
    package_meta: &mut PackageMetaMemo,
    followable_extra_roots: &[PathBuf],
    tp_by_rel: &mut HashMap<String, Vec<String>>,
) {
    let chunks = pass2_chunks(miss_entries, batch_mode);
    let reset_between = matches!(
        batch_mode,
        Pass2BatchMode::Fixed(_) | Pass2BatchMode::Locality
    );
    if reset_between && !chunks.is_empty() {
        *db = db.fresh_empty();
    }
    for (batch_idx, chunk) in chunks.iter().enumerate() {
        if batch_idx > 0 {
            *db = db.fresh_empty();
        }
        let program = db.program();
        for entry in chunk.iter() {
            let file = match system_path_to_file(db, &entry.system_path) {
                Ok(f) => f,
                Err(_) => continue,
            };
            let program_file = program.program_file(db, file);
            let source = source_text(db, file);
            let index = line_index(db, file);
            let parsed = parsed_module(db, program_file.python_file(db)).load(db);
            if parsed.has_invalid_syntax() {
                continue;
            }
            let model = SemanticModel::new(db, program_file);
            let ctx = FileCtx {
                db,
                program_file,
                file,
                rel: entry.rel.clone(),
                module: entry.module.clone(),
                source: source.as_str().to_string(),
                index,
            };
            let module_constants = collect_module_constants(parsed.suite());
            let tp_before = facts.unresolved_third_party_modules.len();
            let mut visitor = CallCollector {
                ctx: &ctx,
                model: &model,
                facts,
                def_map,
                def_map_base: None,
                qual_map,
                qual_map_base: None,
                property_defs,
                task_targets,
                follow_targets,
                package_meta,
                followable_extra_roots,
                record_unresolved_imports: true,
                module_constants: &module_constants,
                current_fn: None,
                current_param_names: Vec::new(),
                class_stack: Vec::new(),
                fn_stack: Vec::new(),
                dead_depth: 0,
                loop_depth: 0,
                branch_depth: 0,
                cache_miss_depth: 0,
                active_param_guard: None,
                consuming_generator: false,
                in_lambda: false,
                skip_lambda_body: false,
                in_callee_position: false,
                pending_is_awaited: false,
                pending_with_enter: false,
                in_eager_offload_arg: false,
                async_with_receivers: HashMap::new(),
                redis_family_memo: HashMap::new(),
                occurrence_counter: HashMap::new(),
                pending_literals: Vec::new(),
                pending_callables: Vec::new(),
                pending_keyword_callables: Vec::new(),
                narrowed_locals: HashMap::new(),
                local_lambdas: HashMap::new(),
                local_genexprs: HashMap::new(),
            };
            for stmt in parsed.suite() {
                visitor.visit_stmt(stmt);
            }
            let file_tp = facts.unresolved_third_party_modules[tp_before..].to_vec();
            tp_by_rel.insert(entry.rel.clone(), file_tp);
        }
    }
}

fn pass2_chunks<'a>(miss_entries: &[&'a FileRef], mode: Pass2BatchMode) -> Vec<Vec<&'a FileRef>> {
    if miss_entries.is_empty() {
        return Vec::new();
    }
    match mode {
        Pass2BatchMode::None => vec![miss_entries.to_vec()],
        Pass2BatchMode::Fixed(n) => miss_entries.chunks(n.max(1)).map(|c| c.to_vec()).collect(),
        Pass2BatchMode::Locality => {
            let mut out: Vec<Vec<&FileRef>> = Vec::new();
            let mut cur: Vec<&FileRef> = Vec::new();
            let mut cur_key: Option<&str> = None;
            for entry in miss_entries {
                let key = locality_key(&entry.rel);
                if cur_key.is_some_and(|k| k != key) && !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
                cur_key = Some(key);
                cur.push(entry);
            }
            if !cur.is_empty() {
                out.push(cur);
            }
            out
        }
    }
}

fn locality_key(rel: &str) -> &str {
    rel.split(['/', '\\'])
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or(rel)
}
