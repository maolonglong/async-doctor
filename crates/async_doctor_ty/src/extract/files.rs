//! Orchestrate multi-pass extraction over first-party Python files.
//!
//! Pass 1 (function discovery) and pass 2 (typed call edges) use separate
//! [`AsyncDoctorDb`] lifetimes. Pass 2 opens the DB in batches so salsa/ty
//! memory from earlier batches can be released — peak RSS tracks one batch
//! plus on-demand imports, not the whole corpus at once.

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
use super::path::{module_name_from_relative, relative_display};

/// Files per pass-2 DB before the salsa database is dropped and reopened.
///
/// Tuned so litellm-sized corpora stay well under ~1 GiB peak RSS while keeping
/// cold/warm JSON byte-identical on the public corpora and fixtures.
/// Larger batches reset salsa less often (faster cold) but retain more state.
/// 280 keeps litellm peak RSS under ~1 GiB on this agent VM while staying
/// byte-identical to the single-DB extract.
const PASS2_DB_BATCH: usize = 280;

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

    // Catalog paths without a DB; relative names match `project_root`.
    let mut catalog: Vec<FileRef> = Vec::with_capacity(files.len());
    for path in files {
        let system_path = match SystemPathBuf::from_path_buf(path.clone()) {
            Ok(p) => p,
            Err(_) => {
                facts.complete = false;
                facts
                    .skipped
                    .push(format!("{}: path is not valid UTF-8", path.display()));
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

    let mut package_meta = PackageMetaMemo::new();
    let mut def_map: HashMap<DefKey, FunctionId> = HashMap::new();
    let mut qual_map: HashMap<(String, String), FunctionId> = HashMap::new();
    let mut property_defs: HashSet<DefKey> = HashSet::new();

    // One environment discovery; later batches reuse settings via `fresh_empty`.
    let mut db = AsyncDoctorDb::open(project_root, None)?;
    let followable_extra_roots: Vec<PathBuf> = db
        .followable_extra_roots()
        .iter()
        .map(|p| PathBuf::from(p.as_str()))
        .collect();

    // Pass 1: collect functions (parse only — no SemanticModel).
    {
        let program = db.program();
        let mut class_methods: HashMap<(String, String), Vec<String>> = HashMap::new();
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

        for func in &mut facts.functions {
            if let Some((class, method)) = func.qualname.rsplit_once('.') {
                let key = (func.module.clone(), class.to_string());
                if let Some(methods) = class_methods.get(&key) {
                    let twin = twin_name(method, methods);
                    if let Some(t) = twin {
                        func.async_twins.push((method.to_string(), t));
                    }
                }
            }
        }
    }

    // Pass 2: typed edges in DB batches (fresh salsa storage each batch).
    let mut task_targets = TaskTargets::default();
    let mut follow_targets: HashMap<String, FollowTarget> = HashMap::new();
    // Drop pass-1 salsa state before the first SemanticModel batch.
    db = db.fresh_empty();

    for (batch_idx, chunk) in catalog.chunks(PASS2_DB_BATCH).enumerate() {
        if batch_idx > 0 {
            db = db.fresh_empty();
        }
        let program = db.program();
        for entry in chunk {
            let file = match system_path_to_file(&db, &entry.system_path) {
                Ok(f) => f,
                Err(_) => continue,
            };
            let program_file = program.program_file(&db, file);
            let source = source_text(&db, file);
            let index = line_index(&db, file);
            let parsed = parsed_module(&db, program_file.python_file(&db)).load(&db);
            if parsed.has_invalid_syntax() {
                continue;
            }
            let model = SemanticModel::new(&db, program_file);
            let ctx = FileCtx {
                db: &db,
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
                facts: &mut facts,
                def_map: &mut def_map,
                qual_map: &mut qual_map,
                property_defs: &property_defs,
                task_targets: &mut task_targets,
                follow_targets: &mut follow_targets,
                package_meta: &mut package_meta,
                followable_extra_roots: &followable_extra_roots,
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
        }
    }

    for fid in task_targets.0 {
        let f = &mut facts.functions[fid.index()];
        if f.label == EntryLabel::Other {
            f.label = EntryLabel::Task;
        }
    }

    // Follow on a fresh DB so third-party extract does not inherit pass-2 peak.
    let db = db.fresh_empty();
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

    stabilize_facts(&mut facts);
    Ok(facts)
}

fn stabilize_facts(facts: &mut ProjectFacts) {
    facts
        .unresolved_imports
        .sort_by(|a, b| (&a.file, &a.module, a.line).cmp(&(&b.file, &b.module, b.line)));
    facts.unresolved_third_party_modules.sort();
    facts.unresolved_third_party_modules.dedup();

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
            CalleeRef::Project(id) => {
                format!(
                    "P:{}",
                    project_keys
                        .get(id.index())
                        .map(String::as_str)
                        .unwrap_or("")
                )
            }
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
