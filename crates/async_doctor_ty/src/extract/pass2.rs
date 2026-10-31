//! Parallel first-party call-resolution (pass 2).
//!
//! Each worker clones the salsa [`AsyncDoctorDb`] (shared memo storage — ty
//! supports parallel queries) and extracts one file into an isolated
//! [`Pass2FileResult`]. Results are merged in catalog file order so FunctionIds
//! for lambdas stay byte-identical to a sequential run.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use async_doctor_core::{
    CallEdge, CalleeRef, FunctionFact, FunctionId, ProjectFacts, UnresolvedImport,
};
use rayon::prelude::*;
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

/// Inputs shared read-only across pass-2 workers (cloned Arc per worker for maps
/// that CallCollector mutates with lambda inserts).
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
    pub rel: String,
    pub new_functions: Vec<FunctionFact>,
    pub new_def_keys: Vec<(DefKey, usize)>,
    pub edges: Vec<CallEdge>,
    pub follow_targets: HashMap<String, FollowTarget>,
    pub task_target_ids: HashSet<FunctionId>,
    /// Existing (pre-pass2) functions labeled Route by imperative registration.
    pub route_ids: HashSet<FunctionId>,
    pub sync_route_ids: HashSet<FunctionId>,
    pub unresolved_imports: Vec<UnresolvedImport>,
    pub unresolved_third_party_modules: Vec<String>,
}

/// Run pass 2 in parallel; returned vec is aligned with `files` order.
///
/// Salsa `Storage` is `Send + !Sync` (thread-local ZalsaLocal). Clone one owned
/// DB **per rayon worker chunk** (not per file) on the calling thread, then
/// process that chunk sequentially so SemanticModel memos reuse within a
/// thread — matching ty's parallel-query model without clone storms.
pub(crate) fn run_pass2_parallel(
    db: &AsyncDoctorDb,
    files: &[Pass2FileSpec],
    shared: &Pass2Shared,
) -> Vec<Pass2FileResult> {
    if files.is_empty() {
        return Vec::new();
    }
    let nthreads = rayon::current_num_threads().max(1);
    let chunk_size = files.len().div_ceil(nthreads).max(1);
    // Build (owned db, index range) jobs on this thread — Storage is !Sync.
    let mut jobs: Vec<(AsyncDoctorDb, usize, usize)> = Vec::new();
    let mut start = 0;
    while start < files.len() {
        let end = (start + chunk_size).min(files.len());
        jobs.push((db.clone(), start, end));
        start = end;
    }
    let mut indexed: Vec<(usize, Pass2FileResult)> = jobs
        .into_par_iter()
        .flat_map(|(db, start, end)| {
            files[start..end]
                .iter()
                .enumerate()
                .map(|(offset, spec)| (start + offset, extract_one_file(&db, spec, shared)))
                .collect::<Vec<_>>()
        })
        .collect();
    indexed.sort_by_key(|(i, _)| *i);
    indexed.into_iter().map(|(_, r)| r).collect()
}

fn extract_one_file(
    db: &AsyncDoctorDb,
    spec: &Pass2FileSpec,
    shared: &Pass2Shared,
) -> Pass2FileResult {
    let base_len = shared.functions.len();
    let mut facts = ProjectFacts {
        functions: (*shared.functions).clone(),
        complete: true,
        ..ProjectFacts::default()
    };
    let mut def_map = (*shared.def_map).clone();
    let mut qual_map = (*shared.qual_map).clone();
    let mut follow_targets: HashMap<String, FollowTarget> = HashMap::new();
    let mut task_targets = TaskTargets::default();
    let mut package_meta = PackageMetaMemo::new();

    let empty = Pass2FileResult {
        rel: spec.rel.clone(),
        new_functions: Vec::new(),
        new_def_keys: Vec::new(),
        edges: Vec::new(),
        follow_targets: HashMap::new(),
        task_target_ids: HashSet::new(),
        route_ids: HashSet::new(),
        sync_route_ids: HashSet::new(),
        unresolved_imports: Vec::new(),
        unresolved_third_party_modules: Vec::new(),
    };

    let file = match system_path_to_file(db, &spec.system_path) {
        Ok(f) => f,
        Err(_) => return empty,
    };
    let program = db.program();
    let program_file = program.program_file(db, file);
    let source = source_text(db, file);
    let index = line_index(db, file);
    let parsed = parsed_module(db, program_file.python_file(db)).load(db);
    if parsed.has_invalid_syntax() {
        return empty;
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
        facts: &mut facts,
        def_map: &mut def_map,
        qual_map: &mut qual_map,
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

    let new_functions: Vec<FunctionFact> = facts.functions.drain(base_len..).collect();
    let mut new_def_keys = Vec::new();
    for (i, f) in new_functions.iter().enumerate() {
        // Provisional id was base_len + i.
        let provisional = FunctionId((base_len + i) as u32);
        if let Some((key, _)) = def_map.iter().find(|(_, id)| **id == provisional) {
            new_def_keys.push((key.clone(), i));
        } else {
            // Fallback: reconstruct from file + name_start unknown — use qual map.
            let _ = f;
        }
    }
    // Prefer def_map entries whose FunctionId is in the provisional range.
    new_def_keys.clear();
    for (key, id) in &def_map {
        let idx = id.0 as usize;
        if idx >= base_len {
            new_def_keys.push((key.clone(), idx - base_len));
        }
    }

    let mut route_ids = HashSet::new();
    let mut sync_route_ids = HashSet::new();
    for f in facts.functions.iter().take(base_len) {
        if f.label == async_doctor_core::EntryLabel::Route {
            route_ids.insert(f.id);
        }
        if f.is_sync_route {
            sync_route_ids.insert(f.id);
        }
    }

    Pass2FileResult {
        rel: spec.rel.clone(),
        new_functions,
        new_def_keys,
        edges: facts.edges,
        follow_targets,
        task_target_ids: task_targets.0,
        route_ids,
        sync_route_ids,
        unresolved_imports: facts.unresolved_imports,
        unresolved_third_party_modules: facts.unresolved_third_party_modules,
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
