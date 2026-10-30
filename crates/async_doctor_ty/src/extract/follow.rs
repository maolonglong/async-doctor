//! Follow reachable third-party site-packages callees into the call graph.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::Path;

use async_doctor_core::{CalleeRef, EntryLabel, FunctionFact, FunctionId, ProjectFacts, SinkTable};
use async_doctor_rules::{BuiltinSinkTable, is_async_counterpart};
use ruff_db::files::system_path_to_file;
use ruff_db::parsed::parsed_module;
use ruff_db::source::{line_index, source_text};
use ruff_python_ast::{self as ast, Stmt};
use ruff_source_file::OneIndexed;
use ty_python_semantic::SemanticModel;

use crate::cache::{effective_cache_root, third_party_module_path};
use crate::db::AsyncDoctorDb;
use crate::sitepkgs::{
    FollowTarget, PackageCacheBatch, PackageMetaMemo, materialize_cached_functions,
};

use super::collector::CallCollector;
use super::constants::body_has_yield;
use super::ctx::{DefKey, FileCtx, TaskTargets};
use super::path::module_name_from_relative;

/// Build an adjacency index: caller → edge indices. O(E) once, O(degree) lookup after.
pub(crate) fn edges_by_caller(facts: &ProjectFacts) -> Vec<Vec<usize>> {
    let mut idx = vec![Vec::new(); facts.functions.len()];
    for (i, edge) in facts.edges.iter().enumerate() {
        let c = edge.caller.index();
        if c < idx.len() {
            idx[c].push(i);
        }
    }
    idx
}

/// Append newly added edges (and newly allocated function slots) into the index.
/// O(ΔE + ΔF) — never rescans the full edge list.
pub(crate) fn extend_edges_by_caller(
    facts: &ProjectFacts,
    idx: &mut Vec<Vec<usize>>,
    from_edge: usize,
) {
    if idx.len() < facts.functions.len() {
        idx.resize(facts.functions.len(), Vec::new());
    }
    for i in from_edge..facts.edges.len() {
        let c = facts.edges[i].caller.index();
        if c < idx.len() {
            idx[c].push(i);
        }
    }
}

/// Default budget for following calls into third-party code after the first wave.
pub(crate) const DEFAULT_FOLLOW_BUDGET: usize = 256;

/// After first-party extraction, follow External callees into site-packages `.py` sources.
///
/// Determinism: BTreeSet/BTreeMap everywhere; sorted queues; sink-table Externals are
/// **never** rewritten to Project (so rule-table findings cannot be dropped by follow).
/// Budget applies only to non-sink follow candidates, sorted by (caller_count desc, qualname).
///
/// `follow_budget` caps how many third-party functions may be followed after the
/// initial first-party-reachable wave. Cold extract and warm cache materialization
/// must charge this identically — never transitively materialize past the cut.
///
/// `package_meta` is the eighth argument (per-scan memo); kept flat so cold/warm
/// call sites stay obvious rather than hiding behind a scratch struct.
#[allow(clippy::too_many_arguments)]
pub(crate) fn follow_third_party(
    db: &AsyncDoctorDb,
    facts: &mut ProjectFacts,
    def_map: &mut HashMap<DefKey, FunctionId>,
    qual_map: &mut HashMap<(String, String), FunctionId>,
    follow_targets: &mut HashMap<String, FollowTarget>,
    package_meta: &mut PackageMetaMemo,
    cache_dir: Option<&Path>,
    follow_budget: usize,
) {
    let sinks = BuiltinSinkTable::new();
    let mut qual_to_id: BTreeMap<String, FunctionId> = BTreeMap::new();
    let mut id_to_external_key: HashMap<FunctionId, String> = HashMap::new();
    let mut cache_batch = PackageCacheBatch::new(effective_cache_root(cache_dir));
    let mut by_caller = edges_by_caller(facts);

    // Forward BFS from async entries over Project edges (BTreeSet worklist = sorted).
    let mut reachable_fns: BTreeSet<FunctionId> = BTreeSet::new();
    let mut work: BTreeSet<FunctionId> = facts
        .functions
        .iter()
        .filter(|f| f.is_async && !f.is_sync_route)
        .map(|f| f.id)
        .collect();
    reachable_fns.extend(work.iter().copied());

    let mut pending_externals: BTreeSet<String> = BTreeSet::new();

    while let Some(&fid) = work.iter().next() {
        work.remove(&fid);
        let Some(edge_ids) = by_caller.get(fid.index()) else {
            continue;
        };
        for &ei in edge_ids {
            let edge = &facts.edges[ei];
            if edge.is_offload_wrapper {
                continue;
            }
            match &edge.callee {
                CalleeRef::Project(id) => {
                    if reachable_fns.insert(*id) {
                        work.insert(*id);
                    }
                }
                CalleeRef::External { qualname } => {
                    // Known async APIs (name rules): never follow. Unknown
                    // third-party `async def` (e.g. libz.fetch → time.sleep) still
                    // follows; structural `callee_is_coroutine` blocks *seeding*
                    // async callees that merely look like sync sinks by name.
                    if edge.is_awaited && is_async_counterpart(qualname) {
                        continue;
                    }
                    // Known sinks stay External forever — never follow/rewrite them.
                    if sinks.lookup(qualname).is_some() {
                        continue;
                    }
                    pending_externals.insert(qualname.clone());
                }
                _ => {}
            }
        }
    }

    // Group by package+fingerprint (BTreeMap = sorted).
    let mut by_package: BTreeMap<(String, String, String), Vec<String>> = BTreeMap::new();
    let mut no_meta: Vec<String> = Vec::new();
    for q in &pending_externals {
        let Some(target) = follow_targets.get(q) else {
            continue;
        };
        match (
            &target.package_name,
            &target.package_version,
            &target.package_fingerprint,
        ) {
            (Some(n), Some(v), Some(fp)) => {
                by_package
                    .entry((n.clone(), v.clone(), fp.clone()))
                    .or_default()
                    .push(q.clone());
            }
            _ => no_meta.push(q.clone()),
        }
    }
    for wanted in by_package.values_mut() {
        wanted.sort();
        wanted.dedup();
    }
    no_meta.sort();
    no_meta.dedup();

    for ((pkg, ver, fp), wanted) in &by_package {
        let edge_before = facts.edges.len();
        let loaded = materialize_cached_functions(
            facts,
            &mut cache_batch,
            pkg,
            ver,
            fp,
            wanted,
            &mut qual_to_id,
            &mut id_to_external_key,
            follow_targets,
        );
        extend_edges_by_caller(facts, &mut by_caller, edge_before);
        for q in &loaded {
            if let Some(id) = qual_to_id.get(q) {
                reachable_fns.insert(*id);
            }
        }
        for q in wanted {
            if qual_to_id.contains_key(q) {
                continue;
            }
            let Some(target) = follow_targets.get(q).cloned() else {
                continue;
            };
            let edge_before = facts.edges.len();
            if let Some(id) = extract_third_party_function(
                db,
                facts,
                def_map,
                qual_map,
                follow_targets,
                package_meta,
                &target,
            ) {
                qual_to_id.insert(q.clone(), id);
                id_to_external_key.insert(id, q.clone());
                reachable_fns.insert(id);
                extend_edges_by_caller(facts, &mut by_caller, edge_before);
                if let (Some(pn), Some(pv), Some(pfp)) = (
                    facts.functions[id.index()].package_name.clone(),
                    facts.functions[id.index()].package_version.clone(),
                    target.package_fingerprint.clone(),
                ) {
                    cache_batch.upsert_function(
                        &pn,
                        &pv,
                        &pfp,
                        q,
                        &facts.functions[id.index()],
                        &facts.edges,
                        &id_to_external_key,
                        follow_targets,
                    );
                }
            }
        }
    }

    for q in &no_meta {
        if qual_to_id.contains_key(q) {
            continue;
        }
        let Some(target) = follow_targets.get(q).cloned() else {
            continue;
        };
        let edge_before = facts.edges.len();
        if let Some(id) = extract_third_party_function(
            db,
            facts,
            def_map,
            qual_map,
            follow_targets,
            package_meta,
            &target,
        ) {
            qual_to_id.insert(q.clone(), id);
            id_to_external_key.insert(id, q.clone());
            reachable_fns.insert(id);
            extend_edges_by_caller(facts, &mut by_caller, edge_before);
        }
    }

    // Rewrite after the initial package/cache wave.
    rewrite_external_to_project(facts, &qual_to_id, &sinks);

    // Further waves until fixpoint or budget — a single extra wave misses deep
    // site-packages chains (liba→libb→libc→libd→sleep). Cold extract and warm
    // cache hits must charge `follow_budget` the same way (one unit per
    // materialized/extracted function, no free transitive closure).
    let mut followed: usize = 0;
    loop {
        let mut wave_candidates: BTreeMap<String, u32> = BTreeMap::new();
        for &fid in &reachable_fns {
            let Some(edge_ids) = by_caller.get(fid.index()) else {
                continue;
            };
            for &ei in edge_ids {
                let edge = &facts.edges[ei];
                if edge.is_offload_wrapper {
                    continue;
                }
                let CalleeRef::External { qualname } = &edge.callee else {
                    continue;
                };
                if edge.is_awaited && is_async_counterpart(qualname) {
                    continue;
                }
                if sinks.lookup(qualname).is_some() {
                    continue;
                }
                if qual_to_id.contains_key(qualname) {
                    continue;
                }
                if !follow_targets.contains_key(qualname) {
                    continue;
                }
                *wave_candidates.entry(qualname.clone()).or_insert(0) += 1;
            }
        }
        if wave_candidates.is_empty() {
            break;
        }
        // Relevance: more callers first, then qualname ascending.
        let mut ranked: Vec<(u32, String)> =
            wave_candidates.into_iter().map(|(q, n)| (n, q)).collect();
        ranked.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
        let remaining = follow_budget.saturating_sub(followed);
        if remaining == 0 {
            break;
        }
        ranked.truncate(remaining);

        let mut progressed = false;
        // Try cache hits for this wave's packages before extracting.
        let mut wave_by_pkg: BTreeMap<(String, String, String), Vec<String>> = BTreeMap::new();
        let mut wave_no_meta: Vec<String> = Vec::new();
        for (_count, q) in &ranked {
            let Some(target) = follow_targets.get(q) else {
                continue;
            };
            match (
                &target.package_name,
                &target.package_version,
                &target.package_fingerprint,
            ) {
                (Some(n), Some(v), Some(fp)) => {
                    wave_by_pkg
                        .entry((n.clone(), v.clone(), fp.clone()))
                        .or_default()
                        .push(q.clone());
                }
                _ => wave_no_meta.push(q.clone()),
            }
        }
        for ((pkg, ver, fp), wanted) in &wave_by_pkg {
            let edge_before = facts.edges.len();
            let loaded = materialize_cached_functions(
                facts,
                &mut cache_batch,
                pkg,
                ver,
                fp,
                wanted,
                &mut qual_to_id,
                &mut id_to_external_key,
                follow_targets,
            );
            if !loaded.is_empty() {
                progressed = true;
                extend_edges_by_caller(facts, &mut by_caller, edge_before);
                for q in &loaded {
                    if let Some(id) = qual_to_id.get(q) {
                        reachable_fns.insert(*id);
                    }
                    followed += 1;
                }
            }
        }

        for (_count, qualname) in ranked {
            if qual_to_id.contains_key(&qualname) {
                continue;
            }
            if followed >= follow_budget {
                break;
            }
            let Some(target) = follow_targets.get(&qualname).cloned() else {
                continue;
            };
            let edge_before = facts.edges.len();
            if let Some(id) = extract_third_party_function(
                db,
                facts,
                def_map,
                qual_map,
                follow_targets,
                package_meta,
                &target,
            ) {
                qual_to_id.insert(qualname.clone(), id);
                id_to_external_key.insert(id, qualname.clone());
                reachable_fns.insert(id);
                extend_edges_by_caller(facts, &mut by_caller, edge_before);
                followed += 1;
                progressed = true;
                if let (Some(pn), Some(pv), Some(pfp)) = (
                    facts.functions[id.index()].package_name.clone(),
                    facts.functions[id.index()].package_version.clone(),
                    target.package_fingerprint.clone(),
                ) {
                    cache_batch.upsert_function(
                        &pn,
                        &pv,
                        &pfp,
                        &qualname,
                        &facts.functions[id.index()],
                        &facts.edges,
                        &id_to_external_key,
                        follow_targets,
                    );
                }
            }
        }
        for q in &wave_no_meta {
            if qual_to_id.contains_key(q) || followed >= follow_budget {
                continue;
            }
            let Some(target) = follow_targets.get(q).cloned() else {
                continue;
            };
            let edge_before = facts.edges.len();
            if let Some(id) = extract_third_party_function(
                db,
                facts,
                def_map,
                qual_map,
                follow_targets,
                package_meta,
                &target,
            ) {
                qual_to_id.insert(q.clone(), id);
                id_to_external_key.insert(id, q.clone());
                reachable_fns.insert(id);
                extend_edges_by_caller(facts, &mut by_caller, edge_before);
                followed += 1;
                progressed = true;
            }
        }

        rewrite_external_to_project(facts, &qual_to_id, &sinks);
        if !progressed {
            break;
        }
    }

    // One atomic write per dirty package.
    cache_batch.flush();
}

/// Rewrite External→Project for extracted callees, skipping known sinks.
pub(crate) fn rewrite_external_to_project(
    facts: &mut ProjectFacts,
    qual_to_id: &BTreeMap<String, FunctionId>,
    sinks: &BuiltinSinkTable,
) {
    for edge in &mut facts.edges {
        let CalleeRef::External { qualname } = &edge.callee else {
            continue;
        };
        if sinks.lookup(qualname).is_some() {
            continue;
        }
        if let Some(id) = qual_to_id.get(qualname) {
            edge.callee = CalleeRef::Project(*id);
        }
    }
}

/// Parse one site-packages function and collect its call edges into `facts`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn extract_third_party_function(
    db: &AsyncDoctorDb,
    facts: &mut ProjectFacts,
    def_map: &mut HashMap<DefKey, FunctionId>,
    qual_map: &mut HashMap<(String, String), FunctionId>,
    follow_targets: &mut HashMap<String, FollowTarget>,
    package_meta: &mut PackageMetaMemo,
    target: &FollowTarget,
) -> Option<FunctionId> {
    let file = system_path_to_file(db, &target.path).ok()?;
    let program = db.program();
    let program_file = program.program_file(db, file);
    let parsed = parsed_module(db, program_file.python_file(db)).load(db);
    if parsed.has_invalid_syntax() {
        return None;
    }
    let source = source_text(db, file);
    let index = line_index(db, file);
    let abs = target.path.as_str().replace('\\', "/");
    let module = third_party_module_path(&abs)
        .map(module_name_from_relative)
        .unwrap_or_default();
    // Stable path so cold extract and cache materialization emit identical JSON.
    // Module already includes the top-level package (`libx.core` → `site-packages/libx/core.py`).
    // System dist-packages use the `dist-packages/` prefix (same third-party policy).
    let tree = if abs.contains("/dist-packages/") {
        "dist-packages"
    } else {
        "site-packages"
    };
    let rel = if !module.is_empty() {
        format!("{tree}/{}.py", module.replace('.', "/"))
    } else if let Some(rest) = third_party_module_path(&abs) {
        format!("{tree}/{rest}")
    } else {
        abs.clone()
    };

    // Find the function node by name_start.
    let mut found: Option<&ast::StmtFunctionDef> = None;
    fn find_fn<'a>(body: &'a [Stmt], name_start: u32, out: &mut Option<&'a ast::StmtFunctionDef>) {
        for stmt in body {
            match stmt {
                Stmt::FunctionDef(f) => {
                    if u32::from(f.name.range.start()) == name_start {
                        *out = Some(f);
                        return;
                    }
                    find_fn(&f.body, name_start, out);
                    if out.is_some() {
                        return;
                    }
                }
                Stmt::ClassDef(c) => {
                    find_fn(&c.body, name_start, out);
                    if out.is_some() {
                        return;
                    }
                }
                Stmt::If(i) => {
                    find_fn(&i.body, name_start, out);
                    for c in &i.elif_else_clauses {
                        find_fn(&c.body, name_start, out);
                    }
                }
                Stmt::For(f) => {
                    find_fn(&f.body, name_start, out);
                    find_fn(&f.orelse, name_start, out);
                }
                Stmt::While(w) => {
                    find_fn(&w.body, name_start, out);
                    find_fn(&w.orelse, name_start, out);
                }
                Stmt::With(w) => find_fn(&w.body, name_start, out),
                Stmt::Try(t) => {
                    find_fn(&t.body, name_start, out);
                    for h in &t.handlers {
                        let ast::ExceptHandler::ExceptHandler(eh) = h;
                        find_fn(&eh.body, name_start, out);
                    }
                    find_fn(&t.orelse, name_start, out);
                    find_fn(&t.finalbody, name_start, out);
                }
                Stmt::Match(m) => {
                    for c in &m.cases {
                        find_fn(&c.body, name_start, out);
                    }
                }
                _ => {}
            }
        }
    }
    find_fn(parsed.suite(), target.name_start, &mut found);
    let func = found?;

    let key = DefKey {
        file: rel.clone(),
        name_start: target.name_start,
    };
    if let Some(id) = def_map.get(&key).copied() {
        return Some(id);
    }

    // Qualname: last segment of target.qualname after module, or function name.
    let qualname = target
        .qualname
        .strip_prefix(&format!("{module}."))
        .unwrap_or(func.name.as_str())
        .to_string();

    let id = FunctionId(facts.functions.len() as u32);
    let param_names: Vec<String> = func
        .parameters
        .iter_non_variadic_params()
        .map(|a| a.parameter.name.as_str().to_string())
        .collect();
    facts.functions.push(FunctionFact {
        id,
        file: rel.clone(),
        line: {
            let line: OneIndexed = index.line_index(func.name.range.start());
            line.get() as u32
        },
        module: module.clone(),
        qualname: qualname.clone(),
        is_async: func.is_async,
        is_generator: body_has_yield(&func.body),
        label: EntryLabel::Other,
        is_sync_route: false,
        async_twins: Vec::new(),
        param_names,
        is_third_party: true,
        package_name: target.package_name.clone(),
        package_version: target.package_version.clone(),
        invokes_params: Vec::new(),
        is_offload_wrapper: false,
    });
    def_map.insert(key, id);
    qual_map.insert((module.clone(), qualname), id);
    let param_names_for_visit = facts.functions[id.index()].param_names.clone();

    let ctx = FileCtx {
        db,
        program_file,
        file,
        rel,
        module,
        source: source.as_str().to_string(),
        index,
    };
    let model = SemanticModel::new(db, program_file);
    let module_constants = HashMap::new();
    let property_defs: HashSet<DefKey> = HashSet::new();
    let mut task_targets = TaskTargets::default();
    let mut visitor = CallCollector {
        ctx: &ctx,
        model: &model,
        facts,
        def_map,
        qual_map,
        property_defs: &property_defs,
        task_targets: &mut task_targets,
        follow_targets,
        package_meta,
        module_constants: &module_constants,
        current_fn: Some(id),
        current_param_names: param_names_for_visit,
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
        occurrence_counter: HashMap::new(),
        pending_literals: Vec::new(),
        pending_callables: Vec::new(),
        pending_keyword_callables: Vec::new(),
        narrowed_locals: HashMap::new(),
        local_lambdas: HashMap::new(),
        local_genexprs: HashMap::new(),
    };
    visitor.visit_body_seq(&func.body);
    Some(id)
}
