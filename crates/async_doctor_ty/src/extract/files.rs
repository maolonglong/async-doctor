//! Orchestrate multi-pass extraction over first-party Python files.
//!
//! `ASYNC_DOCTOR_PASS2_BATCH` controls salsa resets during pass 2:
//! - unset / `0` (default): parallel per-file salsa clones with work-stealing;
//!   one shared `Storage` per worker for the whole pass (no periodic rebuilds)
//! - `locality`: reset between top-level package groups (lower peak RSS, slower)
//! - `N>0`: reset every N files (lower peak RSS, slower cold)

use std::collections::{HashMap, HashSet};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};

use async_doctor_core::{CalleeRef, EntryLabel, FunctionId, ProjectFacts};
use ruff_db::system::{SystemPath, SystemPathBuf};

use crate::db::AsyncDoctorDb;
use crate::sitepkgs::{FollowTarget, PackageMetaMemo};

use super::ctx::{DefKey, TaskTargets};
use super::follow::follow_third_party;
use super::functions::{Pass1FileOutcome, merge_pass1_file_data};
use super::panic_util::panic_message;
use super::pass2::{
    PASS2_WAVE_AST_CLEAR_INTERVAL, Pass2FileResult, Pass2FileSpec, Pass2Shared, extract_one_file,
    extract_pass1_one_file, merge_pass2_results, run_pass1_parallel, run_pass2_parallel,
};
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
    rel: String,
    module: String,
}

pub(crate) fn extract_files(
    project_root: &SystemPath,
    files: Vec<PathBuf>,
    python: Option<&Path>,
    cache_dir: Option<&Path>,
    follow_budget: usize,
) -> anyhow::Result<ProjectFacts> {
    let mut facts = ProjectFacts {
        complete: true,
        ..ProjectFacts::default()
    };

    let mut catalog: Vec<FileRef> = Vec::with_capacity(files.len());
    for path in files {
        let system_path = match SystemPathBuf::from_path_buf(path) {
            Ok(p) => p,
            Err(p) => {
                facts.complete = false;
                facts
                    .skipped
                    .push(format!("{}: path is not valid UTF-8", p.display()));
                continue;
            }
        };
        if crate::cache::path_str_is_third_party_python(system_path.as_str()) {
            continue;
        }
        let rel = relative_display(project_root, &system_path);
        let module = module_name_from_relative(&rel);
        catalog.push(FileRef {
            system_path,
            rel,
            module,
        });
    }
    catalog.sort_by(|a, b| a.rel.cmp(&b.rel));

    let mut def_map: HashMap<DefKey, FunctionId> = HashMap::new();
    let mut qual_map: HashMap<(String, String), FunctionId> = HashMap::new();
    let mut property_defs: HashSet<FunctionId> = HashSet::new();

    let sys_python = python
        .map(crate::env_discovery::absolute_python_path)
        .transpose()?;
    let mut db = AsyncDoctorDb::open(project_root, sys_python.as_deref())?;
    facts.environment = db.scan_environment().clone();
    let mut package_meta = PackageMetaMemo::for_site_packages(&facts.environment.site_packages);
    let followable_extra_roots: Vec<PathBuf> = db
        .followable_extra_roots()
        .iter()
        .map(|p| PathBuf::from(p.as_str()))
        .collect();

    let batch_mode = pass2_batch_mode();
    let mut task_targets = TaskTargets::default();
    let mut follow_targets: HashMap<String, FollowTarget> = HashMap::new();

    if matches!(batch_mode, Pass2BatchMode::None) {
        let pass1_specs: Vec<Pass2FileSpec> = catalog
            .iter()
            .map(|e| Pass2FileSpec {
                system_path: e.system_path.clone(),
                rel: e.rel.clone(),
                module: e.module.clone(),
            })
            .collect();
        let (outcomes, workers) = run_pass1_parallel(&db, &pass1_specs);
        let mut kept_specs: Vec<Pass2FileSpec> = Vec::with_capacity(pass1_specs.len());
        let mut catalog_to_kept: Vec<Option<usize>> = vec![None; pass1_specs.len()];
        for (idx, (spec, outcome)) in pass1_specs.into_iter().zip(outcomes).enumerate() {
            match outcome {
                Pass1FileOutcome::Skipped(msg) => {
                    facts.complete = false;
                    facts.skipped.push(msg);
                }
                Pass1FileOutcome::Ok(data) => {
                    merge_pass1_file_data(
                        &spec.rel,
                        &spec.module,
                        data,
                        &mut facts,
                        &mut def_map,
                        &mut qual_map,
                        &mut property_defs,
                    );
                    catalog_to_kept[idx] = Some(kept_specs.len());
                    kept_specs.push(spec);
                }
            }
        }

        if !kept_specs.is_empty() {
            let base_fn_len = facts.functions.len();
            let results = {
                let shared = Pass2Shared {
                    functions: &facts.functions,
                    def_map: &def_map,
                    qual_map: &qual_map,
                    property_defs: &property_defs,
                    followable_extra_roots: &followable_extra_roots,
                    package_meta: &package_meta,
                };
                run_pass2_parallel(&mut db, workers, &kept_specs, &catalog_to_kept, &shared)
            };
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
            drop(workers);
            db = db.fresh_empty();
        }
    } else {
        // Batched / locality mode: sequential Pass 1 + batched Pass 2.
        const PASS1_RESET_INTERVAL: usize = 512;
        let mut kept: Vec<FileRef> = Vec::with_capacity(catalog.len());
        let mut since_reset = 0usize;
        for entry in catalog {
            if since_reset >= PASS1_RESET_INTERVAL {
                db = db.fresh_empty();
                since_reset = 0;
            }
            let spec = Pass2FileSpec {
                system_path: entry.system_path.clone(),
                rel: entry.rel.clone(),
                module: entry.module.clone(),
            };
            match extract_pass1_one_file(&db, &spec) {
                Pass1FileOutcome::Skipped(msg) => {
                    facts.complete = false;
                    facts.skipped.push(msg);
                }
                Pass1FileOutcome::Ok(data) => {
                    merge_pass1_file_data(
                        &entry.rel,
                        &entry.module,
                        data,
                        &mut facts,
                        &mut def_map,
                        &mut qual_map,
                        &mut property_defs,
                    );
                    kept.push(entry);
                    since_reset += 1;
                }
            }
        }
        db = db.fresh_empty();

        let entries: Vec<&FileRef> = kept.iter().collect();
        run_pass2_batched(
            &mut db,
            &entries,
            batch_mode,
            &mut facts,
            &mut def_map,
            &mut qual_map,
            &property_defs,
            &mut task_targets,
            &mut follow_targets,
            &mut package_meta,
            &followable_extra_roots,
        );
        db = db.fresh_empty();
    }

    for &fid in &task_targets.0 {
        let f = &mut facts.functions[fid.index()];
        if f.label == EntryLabel::Other {
            f.label = EntryLabel::Task;
        }
    }

    follow_third_party(
        &mut db,
        &mut facts,
        &mut def_map,
        &mut qual_map,
        &mut follow_targets,
        &mut package_meta,
        &followable_extra_roots,
        cache_dir,
        follow_budget,
    );
    let drop_db = std::thread::spawn(move || {
        drop(db);
    });

    facts.resolved_symbols = crate::twin_resolve::collect_resolved_twin_symbols_with_python(
        project_root.as_std_path(),
        python,
    );
    stabilize_facts(&mut facts);
    let _ = drop_db.join();
    Ok(facts)
}

#[allow(clippy::too_many_arguments)]
fn run_pass2_batched(
    db: &mut AsyncDoctorDb,
    entries: &[&FileRef],
    batch_mode: Pass2BatchMode,
    facts: &mut ProjectFacts,
    def_map: &mut HashMap<DefKey, FunctionId>,
    qual_map: &mut HashMap<(String, String), FunctionId>,
    property_defs: &HashSet<FunctionId>,
    task_targets: &mut TaskTargets,
    follow_targets: &mut HashMap<String, FollowTarget>,
    package_meta: &mut PackageMetaMemo,
    followable_extra_roots: &[PathBuf],
) {
    if entries.is_empty() {
        return;
    }
    let chunks = pass2_chunks(entries, batch_mode);
    if matches!(
        batch_mode,
        Pass2BatchMode::Fixed(_) | Pass2BatchMode::Locality
    ) {
        *db = db.fresh_empty();
    }
    let base_fn_len = facts.functions.len();
    let results = {
        let shared = Pass2Shared {
            functions: &facts.functions,
            def_map,
            qual_map,
            property_defs,
            followable_extra_roots,
            package_meta,
        };
        let mut out = Vec::with_capacity(entries.len());
        for (batch_idx, chunk) in chunks.iter().enumerate() {
            if batch_idx > 0 {
                db.clear_parsed_modules_only();
                *db = db.fresh_empty();
            }
            for (in_chunk, entry) in chunk.iter().enumerate() {
                let spec = Pass2FileSpec {
                    system_path: entry.system_path.clone(),
                    rel: entry.rel.clone(),
                    module: entry.module.clone(),
                };
                let result =
                    match catch_unwind(AssertUnwindSafe(|| extract_one_file(db, &spec, &shared))) {
                        Ok(r) => r,
                        Err(payload) => {
                            *db = db.fresh_empty();
                            Pass2FileResult::skipped(&entry.rel, &panic_message(&payload))
                        }
                    };
                out.push(result);
                if (in_chunk + 1).is_multiple_of(PASS2_WAVE_AST_CLEAR_INTERVAL) {
                    db.clear_parsed_modules_only();
                }
            }
        }
        out
    };
    merge_pass2_results(
        facts,
        def_map,
        qual_map,
        follow_targets,
        task_targets,
        base_fn_len,
        results,
    );
}

fn pass2_chunks<'a>(entries: &[&'a FileRef], mode: Pass2BatchMode) -> Vec<Vec<&'a FileRef>> {
    if entries.is_empty() {
        return Vec::new();
    }
    match mode {
        Pass2BatchMode::None => vec![entries.to_vec()],
        Pass2BatchMode::Fixed(n) => entries.chunks(n.max(1)).map(|c| c.to_vec()).collect(),
        Pass2BatchMode::Locality => {
            let mut out: Vec<Vec<&FileRef>> = Vec::new();
            let mut cur: Vec<&FileRef> = Vec::new();
            let mut cur_key: Option<&str> = None;
            for entry in entries {
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

fn stabilize_facts(facts: &mut ProjectFacts) {
    facts
        .unresolved_imports
        .sort_by(|a, b| (&a.file, &a.module, a.line).cmp(&(&b.file, &b.module, b.line)));
    facts.unresolved_third_party_modules.sort();
    facts.unresolved_third_party_modules.dedup();
    facts.resolved_third_party_modules.sort();
    facts.resolved_third_party_modules.dedup();

    let functions = &facts.functions;
    let cmp_callee = |ca: &CalleeRef, cb: &CalleeRef| -> std::cmp::Ordering {
        let tag = |c: &CalleeRef| -> u8 {
            match c {
                CalleeRef::External { .. } => b'E',
                CalleeRef::ParamInvoke { .. } => b'I',
                CalleeRef::Offload(_) => b'O',
                CalleeRef::Project(_) => b'P',
                CalleeRef::Unresolved { .. } => b'U',
            }
        };
        tag(ca).cmp(&tag(cb)).then_with(|| match (ca, cb) {
            (CalleeRef::External { qualname: qa }, CalleeRef::External { qualname: qb }) => {
                qa.as_str().cmp(qb.as_str())
            }
            (CalleeRef::ParamInvoke { index: ia }, CalleeRef::ParamInvoke { index: ib }) => {
                if ia == ib {
                    std::cmp::Ordering::Equal
                } else {
                    ia.to_string().cmp(&ib.to_string())
                }
            }
            (CalleeRef::Offload(ka), CalleeRef::Offload(kb)) => ka.as_str().cmp(kb.as_str()),
            (CalleeRef::Project(ida), CalleeRef::Project(idb)) => {
                if ida == idb {
                    return std::cmp::Ordering::Equal;
                }
                let fa = functions.get(ida.index());
                let fb = functions.get(idb.index());
                match (fa, fb) {
                    (Some(fa), Some(fb)) => fa
                        .module
                        .as_str()
                        .cmp(fb.module.as_str())
                        .then_with(|| fa.qualname.as_str().cmp(fb.qualname.as_str())),
                    (Some(_), None) => std::cmp::Ordering::Greater,
                    (None, Some(_)) => std::cmp::Ordering::Less,
                    (None, None) => std::cmp::Ordering::Equal,
                }
            }
            (CalleeRef::Unresolved { reason: ra }, CalleeRef::Unresolved { reason: rb }) => {
                ra.as_str().cmp(rb.as_str())
            }
            _ => std::cmp::Ordering::Equal,
        })
    };
    facts.edges.sort_by(|a, b| {
        (a.file.as_str(), a.line, a.occurrence, a.text.as_str())
            .cmp(&(b.file.as_str(), b.line, b.occurrence, b.text.as_str()))
            .then_with(|| cmp_callee(&a.callee, &b.callee))
            .then_with(|| a.caller.0.cmp(&b.caller.0))
    });
}
