//! Orchestrate multi-pass extraction over first-party Python files.
//!
//! `ASYNC_DOCTOR_PASS2_BATCH` controls salsa resets during pass 2:
//! - unset / `0` (default): never reset; parallel per-file salsa clones
//!   with work-stealing
//! - `locality`: reset between top-level package groups
//! - `N>0`: reset every N files (lower peak RSS, slower cold)

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use async_doctor_core::{CallEdge, CalleeRef, EntryLabel, FunctionId, ProjectFacts};
use ruff_db::files::system_path_to_file;
use ruff_db::parsed::parsed_module;
use ruff_db::source::{line_index, source_text};
use ruff_db::system::{SystemPath, SystemPathBuf};
use ruff_python_ast::visitor::Visitor;
use ty_python_semantic::SemanticModel;

use crate::db::AsyncDoctorDb;
use crate::sitepkgs::{FollowTarget, PackageMetaMemo};

use super::collector::CallCollector;
use super::constants::collect_module_constants;
use super::ctx::{DefKey, FileCtx, TaskTargets};
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
    rel: String,
    module: String,
}

pub(crate) fn extract_files(
    project_root: &SystemPath,
    files: Vec<PathBuf>,
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
        let rel = relative_display(project_root, &system_path);
        let module = module_name_from_relative(&rel);
        catalog.push(FileRef {
            system_path,
            rel,
            module,
        });
    }
    catalog.sort_by(|a, b| a.rel.cmp(&b.rel));

    let mut package_meta = PackageMetaMemo::new();
    let mut def_map: HashMap<DefKey, FunctionId> = HashMap::new();
    let mut qual_map: HashMap<(String, String), FunctionId> = HashMap::new();
    let mut property_defs: HashSet<DefKey> = HashSet::new();
    let mut class_methods: HashMap<(String, String), Vec<String>> = HashMap::new();

    let mut db = AsyncDoctorDb::open(project_root, None)?;
    facts.environment = db.scan_environment().clone();
    let followable_extra_roots: Vec<PathBuf> = db
        .followable_extra_roots()
        .iter()
        .map(|p| PathBuf::from(p.as_str()))
        .collect();

    // Pass 1: collect functions (parse only — no SemanticModel).
    {
        let program = db.program();
        let mut kept: Vec<FileRef> = Vec::with_capacity(catalog.len());
        for entry in catalog {
            let file = match system_path_to_file(&db, &entry.system_path) {
                Ok(f) => f,
                Err(err) => {
                    facts.complete = false;
                    facts
                        .skipped
                        .push(format!("{}: {err}", entry.system_path.as_str()));
                    continue;
                }
            };
            let program_file = program.program_file(&db, file);
            let parsed = parsed_module(&db, program_file.python_file(&db)).load(&db);
            if parsed.has_invalid_syntax() {
                facts.complete = false;
                let detail = parsed
                    .errors()
                    .first()
                    .map(|e| e.to_string())
                    .unwrap_or_else(|| "syntax error".to_string());
                facts.skipped.push(format!("{}: {detail}", entry.rel));
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
            kept.push(entry);
        }
        catalog = kept;
    }

    for func in &mut facts.functions {
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
    let entries: Vec<&FileRef> = catalog.iter().collect();
    let batch_mode = pass2_batch_mode();
    if matches!(batch_mode, Pass2BatchMode::None) && !entries.is_empty() {
        let base_fn_len = facts.functions.len();
        let shared = Pass2Shared {
            functions: std::sync::Arc::new(facts.functions.clone()),
            def_map: std::sync::Arc::new(def_map.clone()),
            qual_map: std::sync::Arc::new(qual_map.clone()),
            property_defs: std::sync::Arc::new(property_defs.clone()),
            followable_extra_roots: std::sync::Arc::new(followable_extra_roots.clone()),
        };
        let specs: Vec<Pass2FileSpec> = entries
            .iter()
            .map(|e| Pass2FileSpec {
                system_path: e.system_path.clone(),
                rel: e.rel.clone(),
                module: e.module.clone(),
            })
            .collect();
        let results = run_pass2_parallel(&db, &specs, &shared);
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
    }

    for &fid in &task_targets.0 {
        let f = &mut facts.functions[fid.index()];
        if f.label == EntryLabel::Other {
            f.label = EntryLabel::Task;
        }
    }

    // Drop first-party SemanticModel memos before follow for stable RSS/identity.
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

    facts.resolved_symbols =
        crate::twin_resolve::collect_resolved_twin_symbols(project_root.as_std_path());
    stabilize_facts(&mut facts);
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
    property_defs: &HashSet<DefKey>,
    task_targets: &mut TaskTargets,
    follow_targets: &mut HashMap<String, FollowTarget>,
    package_meta: &mut PackageMetaMemo,
    followable_extra_roots: &[PathBuf],
) {
    let chunks = pass2_chunks(entries, batch_mode);
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
            super::collector::loop_guards::collect_idle_loop_helpers(
                parsed.suite(),
                &mut visitor.idle_loop_helpers,
            );
            for stmt in parsed.suite() {
                visitor.visit_stmt(stmt);
            }
        }
    }
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
