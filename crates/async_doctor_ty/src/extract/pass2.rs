//! Parallel and per-file isolated first-party call-resolution (Pass 1 & Pass 2).
//!
//! Salsa `Storage` is `Send + !Sync`, so each worker gets an owned DB clone
//! created on the calling thread, then work-steals file indices from a shared
//! queue (per-file granularity). Shared def/qual tables are read through base
//! overlays so workers and batched serial runs never clone or partially mutate
//! shared tables per file.

mod merge;

pub(crate) use merge::{BaseFnPatch, Pass2FileResult, merge_pass2_results};

use std::collections::{HashMap, HashSet, VecDeque};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::sync::Mutex;

use async_doctor_core::{FunctionFact, FunctionId, ProjectFacts};
use ruff_db::files::system_path_to_file;
use ruff_db::parsed::parsed_module;
use ruff_db::source::{line_index, source_text};
use ruff_db::system::SystemPathBuf;
use ruff_python_ast::visitor::Visitor;
use ty_python_semantic::SemanticModel;

use crate::db::AsyncDoctorDb;
use crate::sitepkgs::{FollowTarget, PackageMetaMemo};

use super::collector::CallCollector;
use super::constants::collect_module_constants;
use super::ctx::{DefKey, FileCtx, TaskTargets};
use super::functions::{Pass1FileOutcome, collect_functions_standalone};
use super::panic_util::panic_message;

/// Cap default Pass-1 / Pass-2 worker count. Beyond 8 threads, Salsa shard lock
/// contention and per-thread allocator magazines add hundreds of MB of RSS with
/// no wall-time gain.
const DEFAULT_MAX_PASS_THREADS: usize = 8;

/// Drain newly read paths and clear `ParsedModule` ASTs every N files per wave
/// so third-party `site-packages` ASTs loaded by `semantic_index` do not accumulate.
pub(crate) const PASS2_WAVE_AST_CLEAR_INTERVAL: usize = 200;

/// Maximum first-party files processed per shared `Arc<Zalsa>` wave in Pass 2.
/// Below this threshold (normal repos and the 400-file `db_resets` guardrail),
/// Pass 2 runs in a single wave with zero intermediate `fresh_empty()` resets.
/// Above this threshold (large monorepos), coordinated waves drop the completed
/// wave's `Arc<Zalsa>` completely (refcount = 0) before starting the next wave.
pub(crate) const PASS2_WAVE_FILES: usize = 2500;

/// Worker state carried from parallel Pass 1 into parallel Pass 2 so already
/// parsed `source_text` / `line_index` memos stay warm.
pub(crate) struct Pass1Worker {
    pub(crate) db: AsyncDoctorDb,
    pub(crate) warm_catalog_indices: Vec<usize>,
}

/// Parse one file in Pass 1 and collect its function definitions.
pub(crate) fn extract_pass1_one_file(db: &AsyncDoctorDb, spec: &Pass2FileSpec) -> Pass1FileOutcome {
    let file = match system_path_to_file(db, &spec.system_path) {
        Ok(f) => f,
        Err(err) => {
            return Pass1FileOutcome::Skipped(format!("{}: {err}", spec.system_path.as_str()));
        }
    };
    let program = db.program();
    let program_file = program.program_file(db, file);
    let source = source_text(db, file);
    if let Some(err) = source.read_error() {
        return Pass1FileOutcome::Skipped(format!("{}: {err}", spec.rel));
    };
    let parsed = parsed_module(db, program_file.python_file(db)).load(db);
    if parsed.has_invalid_syntax() {
        let detail = parsed
            .errors()
            .first()
            .map(|e| e.to_string())
            .unwrap_or_else(|| "syntax error".to_string());
        parsed.module().clear();
        return Pass1FileOutcome::Skipped(format!("{}: {detail}", spec.rel));
    }
    let index = line_index(db, file);
    let out = collect_functions_standalone(&index, parsed.suite());
    parsed.module().clear();
    Pass1FileOutcome::Ok(out)
}

/// Run Pass 1 in parallel across worker threads, returning ordered per-file
/// outcomes and the warm per-worker Salsa databases for reuse in Pass 2.
pub(crate) fn run_pass1_parallel(
    db: &AsyncDoctorDb,
    files: &[Pass2FileSpec],
) -> (Vec<Pass1FileOutcome>, Vec<Pass1Worker>) {
    if files.is_empty() {
        return (Vec::new(), Vec::new());
    }
    let nthreads = std::thread::available_parallelism()
        .map(|n| n.get().min(DEFAULT_MAX_PASS_THREADS))
        .unwrap_or(1)
        .min(files.len())
        .max(1);
    let queue = Mutex::new((0..files.len()).collect::<VecDeque<_>>());
    let mut indexed: Vec<(usize, Pass1FileOutcome)> = Vec::with_capacity(files.len());
    let mut workers: Vec<Pass1Worker> = Vec::with_capacity(nthreads);

    std::thread::scope(|scope| {
        let mut handles = Vec::with_capacity(nthreads);
        for _ in 0..nthreads {
            let worker_db = db.clone();
            let queue = &queue;
            handles.push(scope.spawn(move || {
                let mut out = Vec::new();
                let mut warm_catalog_indices = Vec::new();
                loop {
                    let Some(i) = queue.lock().unwrap().pop_front() else {
                        break;
                    };
                    let outcome = extract_pass1_one_file(&worker_db, &files[i]);
                    if matches!(outcome, Pass1FileOutcome::Ok(_)) {
                        warm_catalog_indices.push(i);
                    }
                    out.push((i, outcome));
                }
                (
                    Pass1Worker {
                        db: worker_db,
                        warm_catalog_indices,
                    },
                    out,
                )
            }));
        }
        for h in handles {
            let (worker, out) = h.join().expect("pass1 worker");
            workers.push(worker);
            indexed.extend(out);
        }
    });

    indexed.sort_by_key(|(i, _)| *i);
    let outcomes = indexed.into_iter().map(|(_, o)| o).collect();
    (outcomes, workers)
}

/// Inputs shared read-only across Pass-2 workers and batched serial chunks.
pub(crate) struct Pass2Shared<'a> {
    pub functions: &'a [FunctionFact],
    pub def_map: &'a HashMap<DefKey, FunctionId>,
    pub qual_map: &'a HashMap<(String, String), FunctionId>,
    pub property_defs: &'a HashSet<FunctionId>,
    pub followable_extra_roots: &'a [PathBuf],
    pub package_meta: &'a PackageMetaMemo,
}

pub(crate) struct Pass2FileSpec {
    pub system_path: SystemPathBuf,
    pub rel: String,
    pub module: String,
}

/// Run Pass 2 in parallel with per-thread DBs reused from Pass 1 and affinity
/// work-stealing so files already parsed in a worker's DB hit cached sources.
///
/// For repos with `<= PASS2_WAVE_FILES` (2,500) files, all workers share one
/// `Arc<Zalsa>` for the whole pass (zero intermediate `fresh_empty()` resets)
/// and release it before `follow_third_party`. Larger monorepos run in
/// coordinated waves of `PASS2_WAVE_FILES` so retired arenas drop to refcount 0.
pub(crate) fn run_pass2_parallel(
    base_db: &mut AsyncDoctorDb,
    workers: Vec<Pass1Worker>,
    files: &[Pass2FileSpec],
    catalog_to_kept: &[Option<usize>],
    shared: &Pass2Shared,
) -> Vec<Pass2FileResult> {
    if files.is_empty() || workers.is_empty() {
        drop(workers);
        *base_db = base_db.fresh_empty();
        return Vec::new();
    }
    let nthreads = workers.len();

    if files.len() > PASS2_WAVE_FILES {
        drop(workers);
        let mut indexed: Vec<(usize, Pass2FileResult)> = Vec::with_capacity(files.len());
        let all_indices: Vec<usize> = (0..files.len()).collect();
        for chunk in all_indices.chunks(PASS2_WAVE_FILES) {
            let queue = Mutex::new(chunk.iter().copied().collect::<VecDeque<_>>());
            let wave_count = std::sync::atomic::AtomicUsize::new(0);
            let wave_db_ref = &*base_db;
            std::thread::scope(|scope| {
                let mut handles = Vec::with_capacity(nthreads.min(chunk.len()));
                let queue_ref = &queue;
                let wave_count_ref = &wave_count;
                for _ in 0..nthreads.min(chunk.len()) {
                    let mut worker_db = wave_db_ref.clone();
                    handles.push(scope.spawn(move || {
                        let mut out = Vec::new();
                        loop {
                            let Some(i) = queue_ref.lock().unwrap().pop_front() else {
                                break;
                            };
                            let result = match catch_unwind(AssertUnwindSafe(|| {
                                extract_one_file(&worker_db, &files[i], shared)
                            })) {
                                Ok(r) => r,
                                Err(payload) => {
                                    worker_db = worker_db.fresh_empty();
                                    Pass2FileResult::skipped(
                                        &files[i].rel,
                                        &panic_message(&payload),
                                    )
                                }
                            };
                            out.push((i, result));
                            let done = wave_count_ref
                                .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                                + 1;
                            if done.is_multiple_of(PASS2_WAVE_AST_CLEAR_INTERVAL) {
                                worker_db.clear_parsed_modules_only();
                            }
                        }
                        drop(worker_db);
                        out
                    }));
                }
                for h in handles {
                    indexed.extend(h.join().expect("pass2 wave worker"));
                }
            });
            base_db.clear_parsed_modules_only();
            *base_db = base_db.fresh_empty();
        }
        indexed.sort_by_key(|(i, _)| *i);
        return indexed.into_iter().map(|(_, r)| r).collect();
    }

    let mut worker_deques: Vec<VecDeque<usize>> = vec![VecDeque::new(); nthreads];
    let mut assigned = vec![false; files.len()];
    for (t, w) in workers.iter().enumerate() {
        for &cat_idx in &w.warm_catalog_indices {
            if let Some(Some(kept_idx)) = catalog_to_kept.get(cat_idx)
                && !assigned[*kept_idx]
            {
                assigned[*kept_idx] = true;
                worker_deques[t].push_back(*kept_idx);
            }
        }
    }
    let mut rr = 0usize;
    for (kept_idx, &is_assigned) in assigned.iter().enumerate() {
        if !is_assigned {
            worker_deques[rr % nthreads].push_back(kept_idx);
            rr += 1;
        }
    }
    let queues: Vec<Mutex<VecDeque<usize>>> = worker_deques.into_iter().map(Mutex::new).collect();
    let wave_count = std::sync::atomic::AtomicUsize::new(0);
    let mut indexed: Vec<(usize, Pass2FileResult)> = Vec::with_capacity(files.len());

    std::thread::scope(|scope| {
        let mut handles = Vec::with_capacity(nthreads);
        let queues_ref = &queues;
        let wave_count_ref = &wave_count;
        for (t, worker) in workers.into_iter().enumerate() {
            let mut worker_db = worker.db;
            handles.push(scope.spawn(move || {
                let mut out = Vec::new();
                loop {
                    let mut next_idx = queues_ref[t].lock().unwrap().pop_front();
                    if next_idx.is_none() {
                        for k in 1..nthreads {
                            let victim = (t + k) % nthreads;
                            let stolen = queues_ref[victim].lock().unwrap().pop_back();
                            if stolen.is_some() {
                                next_idx = stolen;
                                break;
                            }
                        }
                    }
                    let Some(i) = next_idx else {
                        break;
                    };
                    let result = match catch_unwind(AssertUnwindSafe(|| {
                        extract_one_file(&worker_db, &files[i], shared)
                    })) {
                        Ok(r) => r,
                        Err(payload) => {
                            worker_db = worker_db.fresh_empty();
                            Pass2FileResult::skipped(&files[i].rel, &panic_message(&payload))
                        }
                    };
                    out.push((i, result));
                    let done =
                        wave_count_ref.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                    if done.is_multiple_of(PASS2_WAVE_AST_CLEAR_INTERVAL) {
                        worker_db.clear_parsed_modules_only();
                    }
                }
                drop(worker_db);
                out
            }));
        }
        for h in handles {
            indexed.extend(h.join().expect("pass2 worker"));
        }
    });
    base_db.clear_parsed_modules_only();
    *base_db = base_db.fresh_empty();

    indexed.sort_by_key(|(i, _)| *i);
    indexed.into_iter().map(|(_, r)| r).collect()
}

/// Test/debug-only: panic when analysing a magic filename or when
/// `ASYNC_DOCTOR_TEST_PANIC_FILE` matches the relative path.
fn maybe_test_panic(rel: &str) {
    #[cfg(any(test, debug_assertions))]
    {
        const MAGIC: &str = "__async_doctor_panic__.py";
        let env_hit = std::env::var("ASYNC_DOCTOR_TEST_PANIC_FILE")
            .ok()
            .filter(|v| !v.is_empty())
            .is_some_and(|v| rel == v || rel.ends_with(&v));
        if env_hit || rel.ends_with(MAGIC) || rel == MAGIC {
            panic!("async-doctor test panic hook");
        }
    }
    let _ = rel;
}

pub(crate) fn extract_one_file(
    db: &AsyncDoctorDb,
    spec: &Pass2FileSpec,
    shared: &Pass2Shared,
) -> Pass2FileResult {
    maybe_test_panic(&spec.rel);
    let base_len = shared.functions.len();

    let file = match system_path_to_file(db, &spec.system_path) {
        Ok(f) => f,
        Err(_) => return Pass2FileResult::empty(),
    };
    let program = db.program();
    let program_file = program.program_file(db, file);
    let source = source_text(db, file);
    if let Some(err) = source.read_error() {
        return Pass2FileResult::skipped(&spec.rel, &err.to_string());
    }
    let index = line_index(db, file);
    let parsed = parsed_module(db, program_file.python_file(db)).load(db);
    if parsed.has_invalid_syntax() {
        return Pass2FileResult::empty();
    }

    let mut facts = ProjectFacts {
        functions: Vec::with_capacity(8),
        complete: true,
        ..ProjectFacts::default()
    };
    let mut def_map: HashMap<DefKey, FunctionId> = HashMap::new();
    let mut qual_map: HashMap<(String, String), FunctionId> = HashMap::new();
    let mut follow_targets: HashMap<String, FollowTarget> = HashMap::new();
    let mut task_targets = TaskTargets::default();
    let mut package_meta = shared.package_meta.fork();

    let model = SemanticModel::new(db, program_file);
    let ctx = FileCtx {
        db,
        program_file,
        file,
        rel: spec.rel.clone(),
        module: spec.module.clone(),
        source,
        index,
    };
    let module_constants = collect_module_constants(parsed.suite());
    let mut visitor = CallCollector::new(
        &ctx,
        &model,
        &mut facts,
        &mut def_map,
        Some(shared.def_map),
        &mut qual_map,
        Some(shared.qual_map),
        shared.property_defs,
        &mut task_targets,
        &mut follow_targets,
        &mut package_meta,
        shared.followable_extra_roots,
        true,
        &module_constants,
    );
    visitor.functions_base = Some(shared.functions);
    super::collector::loop_guards::collect_idle_loop_helpers(
        parsed.suite(),
        &mut visitor.idle_loop_helpers,
    );
    for stmt in parsed.suite() {
        visitor.visit_stmt(stmt);
    }

    let mut modified_base_ids: Vec<FunctionId> = visitor
        .base_offload_wrappers
        .iter()
        .chain(visitor.base_invokes_params.keys())
        .copied()
        .collect();
    modified_base_ids.sort_by_key(|id| id.0);
    modified_base_ids.dedup();
    let mut base_patches = Vec::with_capacity(modified_base_ids.len());
    for id in modified_base_ids {
        let base = &shared.functions[id.index()];
        let is_offload_wrapper =
            base.is_offload_wrapper || visitor.base_offload_wrappers.contains(&id);
        let invokes_params = visitor
            .base_invokes_params
            .remove(&id)
            .unwrap_or_else(|| base.invokes_params.clone());
        base_patches.push(BaseFnPatch {
            id,
            is_offload_wrapper,
            invokes_params,
        });
    }
    let route_ids = visitor.base_route_ids;
    let sync_route_ids = visitor.base_sync_route_ids;

    let new_functions = facts.functions;
    let mut new_def_keys = Vec::with_capacity(def_map.len());
    for (key, id) in &def_map {
        let idx = id.0 as usize;
        if idx >= base_len {
            new_def_keys.push((key.clone(), idx - base_len));
        }
    }

    parsed.module().clear();

    Pass2FileResult {
        new_functions,
        new_def_keys,
        edges: facts.edges,
        follow_targets,
        task_target_ids: task_targets.0,
        route_ids,
        sync_route_ids,
        base_patches,
        unresolved_imports: facts.unresolved_imports,
        unresolved_third_party_modules: facts.unresolved_third_party_modules,
        resolved_third_party_modules: facts.resolved_third_party_modules,
        decorator_refs: facts.decorator_refs,
        offload_wrapper_refs: facts.offload_wrapper_refs,
        skip_reason: None,
    }
}
