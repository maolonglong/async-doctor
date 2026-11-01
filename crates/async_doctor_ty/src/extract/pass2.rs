//! Parallel first-party call-resolution (pass 2).
//!
//! Salsa `Storage` is `Send + !Sync`, so each worker gets an owned DB clone
//! created on the calling thread, then work-steals file indices from a shared
//! queue (per-file granularity). Shared def/qual tables are read through base
//! overlays so workers do not clone those maps per file.

use std::collections::{HashMap, HashSet, VecDeque};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use async_doctor_core::{
    CallEdge, CalleeRef, FunctionFact, FunctionId, ProjectFacts, UnresolvedImport,
};
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
use super::panic_util::panic_message;

/// Inputs shared read-only across pass-2 workers.
pub(crate) struct Pass2Shared {
    pub functions: Arc<Vec<FunctionFact>>,
    pub def_map: Arc<HashMap<DefKey, FunctionId>>,
    pub qual_map: Arc<HashMap<(String, String), FunctionId>>,
    pub property_defs: Arc<HashSet<DefKey>>,
    pub followable_extra_roots: Arc<Vec<PathBuf>>,
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

/// Run pass 2 in parallel with per-thread DB clones and per-file work-stealing.
pub(crate) fn run_pass2_parallel(
    db: &AsyncDoctorDb,
    files: &[Pass2FileSpec],
    shared: &Pass2Shared,
) -> Vec<Pass2FileResult> {
    if files.is_empty() {
        return Vec::new();
    }
    let nthreads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .min(files.len())
        .max(1);
    let queue = Mutex::new((0..files.len()).collect::<VecDeque<_>>());
    let mut indexed: Vec<(usize, Pass2FileResult)> = Vec::with_capacity(files.len());

    std::thread::scope(|scope| {
        let mut handles = Vec::with_capacity(nthreads);
        for _ in 0..nthreads {
            // Clone on this thread — Storage is !Sync so rayon map_init cannot
            // capture `&AsyncDoctorDb`.
            let mut worker_db = db.clone();
            let queue = &queue;
            handles.push(scope.spawn(move || {
                let mut out = Vec::new();
                loop {
                    let i = queue.lock().unwrap().pop_front();
                    let Some(i) = i else {
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
                }
                out
            }));
        }
        for h in handles {
            indexed.extend(h.join().expect("pass2 worker"));
        }
    });

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

    // Functions: copy-on-write scratch (CallCollector mutates base slots).
    // def/qual maps: empty locals + shared base overlays (no full-map clone).
    let mut facts = ProjectFacts {
        functions: Vec::with_capacity(base_len + 8),
        complete: true,
        ..ProjectFacts::default()
    };
    facts.functions.extend(shared.functions.iter().cloned());
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
        source: source.as_str().to_string(),
        index,
    };
    let module_constants = collect_module_constants(parsed.suite());
    let mut visitor = CallCollector {
        ctx: &ctx,
        model: &model,
        facts: &mut facts,
        def_map: &mut def_map,
        def_map_base: Some(shared.def_map.as_ref()),
        qual_map: &mut qual_map,
        qual_map_base: Some(shared.qual_map.as_ref()),
        property_defs: &shared.property_defs,
        task_targets: &mut task_targets,
        follow_targets: &mut follow_targets,
        package_meta: &mut package_meta,
        followable_extra_roots: &shared.followable_extra_roots,
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
        no_running_loop_depth: 0,
        try_saw_get_loop: false,
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
        pending_externals: Vec::new(),
        pending_constructed_receivers: Vec::new(),
        pending_constructed_receiver: None,
        pending_keyword_callables: Vec::new(),
        narrowed_locals: HashMap::new(),
        local_lambdas: HashMap::new(),
        local_genexprs: HashMap::new(),
        expanding_genexprs: HashSet::new(),
        genexpr_expand_depth: 0,
        idle_loop_locals: HashSet::new(),
        idle_loop_helpers: HashSet::new(),
        unbounded_queues: HashSet::new(),
        running_loop_locals: HashSet::new(),
        event_loop_locals: HashSet::new(),
    };
    super::collector::loop_guards::collect_idle_loop_helpers(
        parsed.suite(),
        &mut visitor.idle_loop_helpers,
    );
    for stmt in parsed.suite() {
        visitor.visit_stmt(stmt);
    }

    let mut base_patches = Vec::new();
    let mut route_ids = HashSet::new();
    let mut sync_route_ids = HashSet::new();
    for (i, f) in facts.functions.iter().enumerate().take(base_len) {
        let base = &shared.functions[i];
        if f.label == async_doctor_core::EntryLabel::Route {
            route_ids.insert(f.id);
        }
        if f.is_sync_route {
            sync_route_ids.insert(f.id);
        }
        if f.is_offload_wrapper != base.is_offload_wrapper
            || f.invokes_params != base.invokes_params
        {
            base_patches.push(BaseFnPatch {
                id: f.id,
                is_offload_wrapper: f.is_offload_wrapper,
                invokes_params: f.invokes_params.clone(),
            });
        }
    }

    let new_functions: Vec<FunctionFact> = facts.functions.drain(base_len..).collect();
    let mut new_def_keys = Vec::with_capacity(def_map.len());
    for (key, id) in &def_map {
        let idx = id.0 as usize;
        if idx >= base_len {
            new_def_keys.push((key.clone(), idx - base_len));
        }
    }

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
        source: source.as_str().to_string(),
        index,
    };
    let module_constants = collect_module_constants(parsed.suite());
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
        no_running_loop_depth: 0,
        try_saw_get_loop: false,
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
        pending_externals: Vec::new(),
        pending_constructed_receivers: Vec::new(),
        pending_constructed_receiver: None,
        pending_keyword_callables: Vec::new(),
        narrowed_locals: HashMap::new(),
        local_lambdas: HashMap::new(),
        local_genexprs: HashMap::new(),
        expanding_genexprs: HashSet::new(),
        genexpr_expand_depth: 0,
        idle_loop_locals: HashSet::new(),
        idle_loop_helpers: HashSet::new(),
        unbounded_queues: HashSet::new(),
        running_loop_locals: HashSet::new(),
        event_loop_locals: HashSet::new(),
    };
    super::collector::loop_guards::collect_idle_loop_helpers(
        parsed.suite(),
        &mut visitor.idle_loop_helpers,
    );
    for stmt in parsed.suite() {
        visitor.visit_stmt(stmt);
    }
}
