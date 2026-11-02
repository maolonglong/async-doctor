//! Parallel first-party call-resolution (pass 2).
//!
//! Salsa `Storage` is `Send + !Sync`, so each worker gets an owned DB clone
//! created on the calling thread, then work-steals file indices from a shared
//! queue (per-file granularity). Shared def/qual tables are read through base
//! overlays so workers do not clone those maps per file.

use std::collections::{HashMap, HashSet, VecDeque};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::sync::Mutex;

use async_doctor_core::{
    CallEdge, CalleeRef, FunctionFact, FunctionId, ProjectFacts, UnresolvedImport,
};
use ruff_db::files::system_path_to_file;
use ruff_db::parsed::parsed_module;
use ruff_db::source::{line_index, source_text};
use ruff_db::system::SystemPathBuf;
use ruff_python_ast::visitor::Visitor;
use ty_python_semantic::SemanticModel;

use crate::db::{AsyncDoctorDb, release_freed_memory};
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

/// Clear `ParsedModule` ASTs across all read files every N files per worker so
/// third-party `site-packages` ASTs loaded by `semantic_index` do not accumulate.
const PASS2_AST_CLEAR_INTERVAL: usize = 25;

/// Maximum first-party files processed per shared `Arc<Zalsa>` wave in Pass 2.
/// Below this threshold (normal repos and the 400-file `db_resets` guardrail),
/// Pass 2 runs in a single wave with zero intermediate `fresh_empty()` resets.
/// Above this threshold (large monorepos), coordinated waves drop the completed
/// wave's `Arc<Zalsa>` completely (refcount = 0) before starting the next wave,
/// bounding live Salsa `semantic_index` / inference memos to at most one wave.
const PASS2_WAVE_FILES: usize = 1000;

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
///
/// Workers keep a single shared `Storage` clone for the whole pass. Periodic
/// `fresh_empty()` rebuilds were removed from the default path: each rebuild
/// allocates an independent `Arc<Zalsa>`, and holding retired DBs until the
/// end of the scan caused unbounded RSS growth on large monorepos.
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
                    let i = queue.lock().unwrap().pop_front();
                    let Some(i) = i else {
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

/// Inputs shared read-only across pass-2 workers.
pub(crate) struct Pass2Shared<'a> {
    pub functions: &'a [FunctionFact],
    pub def_map: &'a HashMap<DefKey, FunctionId>,
    pub qual_map: &'a HashMap<(String, String), FunctionId>,
    pub property_defs: &'a HashSet<DefKey>,
    pub followable_extra_roots: &'a [PathBuf],
}

pub(crate) struct Pass2FileSpec {
    pub system_path: SystemPathBuf,
    pub rel: String,
    pub module: String,
}

pub(crate) struct Pass2FileResult {
    pub new_functions: Vec<FunctionFact>,
    pub new_def_keys: Vec<(DefKey, usize)>,
    pub edges: Vec<CallEdge>,
    pub follow_targets: HashMap<String, FollowTarget>,
    pub task_target_ids: HashSet<FunctionId>,
    /// Existing (pre-pass2) functions labeled Route by imperative registration.
    pub route_ids: HashSet<FunctionId>,
    pub sync_route_ids: HashSet<FunctionId>,
    /// Base-function field patches applied during this file's visit.
    pub base_patches: Vec<BaseFnPatch>,
    pub unresolved_imports: Vec<UnresolvedImport>,
    pub unresolved_third_party_modules: Vec<String>,
    pub resolved_third_party_modules: Vec<String>,
    /// When set, this file was skipped (panic / hard limit); merge marks incomplete.
    pub skip_reason: Option<String>,
}

/// Mutations CallCollector makes on pre-existing (pass-1) functions.
#[derive(Debug, Clone)]
pub(crate) struct BaseFnPatch {
    pub id: FunctionId,
    pub is_offload_wrapper: bool,
    pub invokes_params: Vec<u32>,
}

/// Run pass 2 in parallel with per-thread DBs reused from pass 1 and affinity
/// work-stealing so files already parsed in a worker's DB hit cached sources.
///
/// For repos with `<= PASS2_WAVE_FILES` (1,000) files, all workers share one
/// `Arc<Zalsa>` for the whole pass (zero intermediate `fresh_empty()` resets)
/// and release it before `follow_third_party`. For larger monorepos, files
/// (sorted by path locality) are processed in coordinated waves of
/// `PASS2_WAVE_FILES` where all workers share one `Arc<Zalsa>` per wave and
/// drop it completely (refcount = 0) before the next wave begins, bounding
/// live Salsa memos without retaining multiple retired arenas.
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
            let wave_db_ref = &*base_db;
            std::thread::scope(|scope| {
                let mut handles = Vec::with_capacity(nthreads.min(chunk.len()));
                let queue_ref = &queue;
                for _ in 0..nthreads.min(chunk.len()) {
                    let mut worker_db = wave_db_ref.clone();
                    handles.push(scope.spawn(move || {
                        let mut out = Vec::new();
                        let mut count = 0usize;
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
                            count += 1;
                            if count.is_multiple_of(PASS2_AST_CLEAR_INTERVAL) {
                                worker_db.clear_all_parsed_modules();
                            }
                        }
                        worker_db.clear_all_parsed_modules();
                        drop(worker_db);
                        out
                    }));
                }
                for h in handles {
                    indexed.extend(h.join().expect("pass2 wave worker"));
                }
            });
            let next_db = base_db.fresh_empty();
            *base_db = next_db;
            release_freed_memory();
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
    let mut indexed: Vec<(usize, Pass2FileResult)> = Vec::with_capacity(files.len());

    std::thread::scope(|scope| {
        let mut handles = Vec::with_capacity(nthreads);
        let queues_ref = &queues;
        for (t, worker) in workers.into_iter().enumerate() {
            let mut worker_db = worker.db;
            handles.push(scope.spawn(move || {
                let mut out = Vec::new();
                let mut count = 0usize;
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
                            // Salsa state may be poisoned — drop memos for the next file.
                            worker_db = worker_db.fresh_empty();
                            Pass2FileResult::skipped(&files[i].rel, &panic_message(&payload))
                        }
                    };
                    out.push((i, result));
                    count += 1;
                    if count.is_multiple_of(PASS2_AST_CLEAR_INTERVAL) {
                        worker_db.clear_all_parsed_modules();
                    }
                }
                worker_db.clear_all_parsed_modules();
                drop(worker_db);
                out
            }));
        }
        for h in handles {
            let out = h.join().expect("pass2 worker");
            indexed.extend(out);
        }
    });
    let next_db = base_db.fresh_empty();
    *base_db = next_db;
    release_freed_memory();

    indexed.sort_by_key(|(i, _)| *i);
    indexed.into_iter().map(|(_, r)| r).collect()
}

impl Pass2FileResult {
    pub(crate) fn empty() -> Self {
        Self {
            new_functions: Vec::new(),
            new_def_keys: Vec::new(),
            edges: Vec::new(),
            follow_targets: HashMap::new(),
            task_target_ids: HashSet::new(),
            route_ids: HashSet::new(),
            sync_route_ids: HashSet::new(),
            base_patches: Vec::new(),
            unresolved_imports: Vec::new(),
            unresolved_third_party_modules: Vec::new(),
            resolved_third_party_modules: Vec::new(),
            skip_reason: None,
        }
    }

    pub(crate) fn skipped(rel: &str, reason: &str) -> Self {
        let mut r = Self::empty();
        r.skip_reason = Some(format!("{rel}: {reason}"));
        r
    }
}

/// Test/debug-only: panic when analysing a magic filename or when
/// `ASYNC_DOCTOR_TEST_PANIC_FILE` matches the relative path. Used to verify
/// per-file isolation without relying on real analyser bugs.
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

    // Functions and def/qual maps: empty locals + shared read-only base overlays
    // (no per-file clone of the full function or def/qual tables).
    let mut facts = ProjectFacts {
        functions: Vec::with_capacity(8),
        complete: true,
        ..ProjectFacts::default()
    };
    let mut def_map: HashMap<DefKey, FunctionId> = HashMap::new();
    let mut qual_map: HashMap<(String, String), FunctionId> = HashMap::new();
    let mut follow_targets: HashMap<String, FollowTarget> = HashMap::new();
    let mut task_targets = TaskTargets::default();
    let mut package_meta = PackageMetaMemo::new();

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
        skip_reason: None,
    }
}

/// Merge parallel file results into `facts` in the same order as `results`.
pub(crate) fn merge_pass2_results(
    facts: &mut ProjectFacts,
    def_map: &mut HashMap<DefKey, FunctionId>,
    qual_map: &mut HashMap<(String, String), FunctionId>,
    follow_targets: &mut HashMap<String, FollowTarget>,
    task_targets: &mut TaskTargets,
    base_fn_len: usize,
    results: Vec<Pass2FileResult>,
) {
    for result in results {
        if let Some(reason) = result.skip_reason {
            facts.complete = false;
            facts.skipped.push(reason);
            continue;
        }
        let mut remap: HashMap<FunctionId, FunctionId> = HashMap::new();
        for (i, mut f) in result.new_functions.into_iter().enumerate() {
            let provisional = FunctionId((base_fn_len + i) as u32);
            let new_id = FunctionId(facts.functions.len() as u32);
            remap.insert(provisional, new_id);
            f.id = new_id;
            qual_map.insert((f.module.clone(), f.qualname.clone()), new_id);
            facts.functions.push(f);
        }
        for (key, local_idx) in result.new_def_keys {
            let provisional = FunctionId((base_fn_len + local_idx) as u32);
            if let Some(&new_id) = remap.get(&provisional) {
                def_map.insert(key, new_id);
            }
        }
        for patch in result.base_patches {
            let id = remap.get(&patch.id).copied().unwrap_or(patch.id);
            if let Some(f) = facts.functions.get_mut(id.index()) {
                f.is_offload_wrapper = patch.is_offload_wrapper;
                f.invokes_params = patch.invokes_params;
            }
        }
        for mut edge in result.edges {
            remap_edge_ids(&mut edge, &remap);
            facts.edges.push(edge);
        }
        for (q, t) in result.follow_targets {
            follow_targets.entry(q).or_insert(t);
        }
        for id in result.task_target_ids {
            let id = remap.get(&id).copied().unwrap_or(id);
            task_targets.0.insert(id);
        }
        for id in result.route_ids {
            let id = remap.get(&id).copied().unwrap_or(id);
            let f = &mut facts.functions[id.index()];
            if f.label == async_doctor_core::EntryLabel::Other {
                f.label = async_doctor_core::EntryLabel::Route;
            }
        }
        for id in result.sync_route_ids {
            let id = remap.get(&id).copied().unwrap_or(id);
            facts.functions[id.index()].is_sync_route = true;
        }
        facts.unresolved_imports.extend(result.unresolved_imports);
        facts
            .unresolved_third_party_modules
            .extend(result.unresolved_third_party_modules);
        facts
            .resolved_third_party_modules
            .extend(result.resolved_third_party_modules);
    }
}

fn remap_edge_ids(edge: &mut CallEdge, remap: &HashMap<FunctionId, FunctionId>) {
    if let Some(&id) = remap.get(&edge.caller) {
        edge.caller = id;
    }
    if let CalleeRef::Project(id) = &mut edge.callee
        && let Some(&nid) = remap.get(id)
    {
        *id = nid;
    }
    for slot in &mut edge.positional_callables {
        if let Some(id) = slot.as_mut()
            && let Some(&nid) = remap.get(id)
        {
            *id = nid;
        }
    }
    for (_, id) in &mut edge.keyword_callables {
        if let Some(&nid) = remap.get(id) {
            *id = nid;
        }
    }
    if let Some(id) = edge.context_from_caller.as_mut()
        && let Some(&nid) = remap.get(id)
    {
        *id = nid;
    }
}

/// Lengths of shared pass-2 buffers before visiting one file (serial path).
pub(crate) struct Pass2FileLens {
    functions: usize,
    edges: usize,
    unresolved_imports: usize,
    unresolved_third_party_modules: usize,
    resolved_third_party_modules: usize,
}

impl Pass2FileLens {
    pub(crate) fn capture(facts: &ProjectFacts) -> Self {
        Self {
            functions: facts.functions.len(),
            edges: facts.edges.len(),
            unresolved_imports: facts.unresolved_imports.len(),
            unresolved_third_party_modules: facts.unresolved_third_party_modules.len(),
            resolved_third_party_modules: facts.resolved_third_party_modules.len(),
        }
    }

    /// Drop mid-file writes after a panic. Base-function field patches are left
    /// as-is (scan is already `complete=false`); map entries pointing past the
    /// truncated function list are removed.
    pub(crate) fn rollback(
        &self,
        facts: &mut ProjectFacts,
        def_map: &mut HashMap<DefKey, FunctionId>,
        qual_map: &mut HashMap<(String, String), FunctionId>,
        task_targets: &mut TaskTargets,
    ) {
        facts.functions.truncate(self.functions);
        facts.edges.truncate(self.edges);
        facts.unresolved_imports.truncate(self.unresolved_imports);
        facts
            .unresolved_third_party_modules
            .truncate(self.unresolved_third_party_modules);
        facts
            .resolved_third_party_modules
            .truncate(self.resolved_third_party_modules);
        def_map.retain(|_, id| id.index() < self.functions);
        qual_map.retain(|_, id| id.index() < self.functions);
        task_targets.0.retain(|id| id.index() < self.functions);
    }
}

/// Serial pass-2 visit: write into the shared maps (no per-file clones). Caller
/// wraps this in `catch_unwind` and rolls back with [`Pass2FileLens`] on panic.
#[allow(clippy::too_many_arguments)]
pub(crate) fn visit_one_file_shared(
    db: &AsyncDoctorDb,
    spec: &Pass2FileSpec,
    facts: &mut ProjectFacts,
    def_map: &mut HashMap<DefKey, FunctionId>,
    qual_map: &mut HashMap<(String, String), FunctionId>,
    property_defs: &HashSet<DefKey>,
    task_targets: &mut TaskTargets,
    follow_targets: &mut HashMap<String, FollowTarget>,
    package_meta: &mut PackageMetaMemo,
    followable_extra_roots: &[PathBuf],
) {
    maybe_test_panic(&spec.rel);
    let file = match system_path_to_file(db, &spec.system_path) {
        Ok(f) => f,
        Err(_) => return,
    };
    let program = db.program();
    let program_file = program.program_file(db, file);
    let source = source_text(db, file);
    if let Some(err) = source.read_error() {
        facts.complete = false;
        facts.skipped.push(format!("{}: {err}", spec.rel));
        return;
    }
    let index = line_index(db, file);
    let parsed = parsed_module(db, program_file.python_file(db)).load(db);
    if parsed.has_invalid_syntax() {
        return;
    }
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
        facts,
        def_map,
        None,
        qual_map,
        None,
        property_defs,
        task_targets,
        follow_targets,
        package_meta,
        followable_extra_roots,
        true,
        &module_constants,
    );
    super::collector::loop_guards::collect_idle_loop_helpers(
        parsed.suite(),
        &mut visitor.idle_loop_helpers,
    );
    for stmt in parsed.suite() {
        visitor.visit_stmt(stmt);
    }
    parsed.module().clear();
}
