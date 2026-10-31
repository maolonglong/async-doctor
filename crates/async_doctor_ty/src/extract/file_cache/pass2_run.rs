//! Pass-2 execution helpers for the incremental file-cache path.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use async_doctor_core::{FunctionId, ProjectFacts};
use ruff_db::files::system_path_to_file;
use ruff_db::parsed::parsed_module;
use ruff_db::source::{line_index, source_text};
use ruff_python_ast::visitor::Visitor;
use ty_python_semantic::SemanticModel;

use crate::db::AsyncDoctorDb;
use crate::sitepkgs::{FollowTarget, PackageMetaMemo};

use crate::extract::collector::CallCollector;
use crate::extract::constants::collect_module_constants;
use crate::extract::ctx::{DefKey, FileCtx, TaskTargets};
use crate::extract::pass2::{Pass2FileSpec, Pass2Shared, merge_pass2_results, run_pass2_parallel};

use super::orchestrate::{FileRef, Pass2BatchMode};

#[allow(clippy::too_many_arguments)]
pub(super) fn run_pass2_for_indices(
    db: &mut AsyncDoctorDb,
    catalog: &[FileRef],
    miss_indices: &HashSet<usize>,
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
    let mut miss_entries: Vec<&FileRef> = miss_indices.iter().map(|&i| &catalog[i]).collect();
    miss_entries.sort_by(|a, b| a.rel.cmp(&b.rel));
    if miss_entries.is_empty() {
        return;
    }
    if matches!(batch_mode, Pass2BatchMode::None) {
        let base_fn_len = facts.functions.len();
        let shared = Pass2Shared {
            functions: std::sync::Arc::new(facts.functions.clone()),
            def_map: std::sync::Arc::new(def_map.clone()),
            qual_map: std::sync::Arc::new(qual_map.clone()),
            property_defs: std::sync::Arc::new(property_defs.clone()),
            followable_extra_roots: std::sync::Arc::new(followable_extra_roots.to_vec()),
        };
        let specs: Vec<Pass2FileSpec> = miss_entries
            .iter()
            .map(|e| Pass2FileSpec {
                system_path: e.system_path.clone(),
                rel: e.rel.clone(),
                module: e.module.clone(),
            })
            .collect();
        let results = run_pass2_parallel(db, &specs, &shared);
        for (spec, r) in specs.iter().zip(results.iter()) {
            tp_by_rel.insert(spec.rel.clone(), r.unresolved_third_party_modules.clone());
        }
        merge_pass2_results(
            facts,
            def_map,
            qual_map,
            follow_targets,
            task_targets,
            base_fn_len,
            results,
        );
    } else {
        run_pass2_batched(
            db,
            &miss_entries,
            batch_mode,
            facts,
            def_map,
            qual_map,
            property_defs,
            task_targets,
            follow_targets,
            package_meta,
            followable_extra_roots,
            tp_by_rel,
        );
    }
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
                idle_loop_locals: HashSet::new(),
                idle_loop_helpers: HashSet::new(),
            };
            crate::extract::collector::loop_guards::collect_idle_loop_helpers(
                parsed.suite(),
                &mut visitor.idle_loop_helpers,
            );
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
