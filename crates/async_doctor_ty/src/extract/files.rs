//! Orchestrate multi-pass extraction over first-party Python files.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use async_doctor_core::{CallEdge, CalleeRef, EntryLabel, FunctionId, ProjectFacts};
use ruff_db::files::{File, system_path_to_file};
use ruff_db::parsed::parsed_module;
use ruff_db::source::{line_index, source_text};
use ruff_db::system::SystemPathBuf;
use ruff_python_ast::visitor::Visitor;
use ty_python_core::ProgramFile;
use ty_python_semantic::SemanticModel;

use crate::db::AsyncDoctorDb;
use crate::sitepkgs::{FollowTarget, PackageMetaMemo};

use super::collector::CallCollector;
use super::constants::collect_module_constants;
use super::ctx::{DefKey, FileCtx, TaskTargets};
use super::follow::follow_third_party;
use super::functions::{collect_functions, twin_name};
use super::path::{module_name_from_relative, relative_display};

pub(crate) fn extract_files(
    db: &AsyncDoctorDb,
    files: Vec<PathBuf>,
    cache_dir: Option<&Path>,
    follow_budget: usize,
) -> anyhow::Result<ProjectFacts> {
    let mut facts = ProjectFacts {
        complete: true,
        ..ProjectFacts::default()
    };
    let mut package_meta = PackageMetaMemo::new();
    let program = db.program();
    let followable_extra_roots: Vec<std::path::PathBuf> = db
        .followable_extra_roots()
        .iter()
        .map(|p| std::path::PathBuf::from(p.as_str()))
        .collect();

    // Open + parse all first-party files first.
    let mut opened: Vec<(SystemPathBuf, File, ProgramFile<'_>, String, String)> = Vec::new();
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
        let file = match system_path_to_file(db, &system_path) {
            Ok(f) => f,
            Err(err) => {
                facts.complete = false;
                facts
                    .skipped
                    .push(format!("{}: {err}", system_path.as_str()));
                continue;
            }
        };
        let program_file = program.program_file(db, file);
        let rel = relative_display(db.project_root(), &system_path);
        let module = module_name_from_relative(&rel);
        let parsed = parsed_module(db, program_file.python_file(db)).load(db);
        if parsed.has_invalid_syntax() {
            facts.complete = false;
            let detail = parsed
                .errors()
                .first()
                .map(|e| e.to_string())
                .unwrap_or_else(|| "syntax error".to_string());
            facts.skipped.push(format!("{rel}: {detail}"));
            continue;
        }
        opened.push((system_path, file, program_file, rel, module));
    }

    // Pass 1: collect functions.
    let mut def_map: HashMap<DefKey, FunctionId> = HashMap::new();
    // Also map module:qualname → FunctionId for same-module lookups.
    let mut qual_map: HashMap<(String, String), FunctionId> = HashMap::new();
    // Track class method names per class key for async-twin hints.
    let mut class_methods: HashMap<(String, String), Vec<String>> = HashMap::new();
    // `@property` getters keyed by definition site.
    let mut property_defs: HashSet<DefKey> = HashSet::new();

    for (_sys, file, program_file, rel, module) in &opened {
        let source = source_text(db, *file);
        let index = line_index(db, *file);
        let parsed = parsed_module(db, program_file.python_file(db)).load(db);
        let ctx = FileCtx {
            db,
            program_file: *program_file,
            file: *file,
            rel: rel.clone(),
            module: module.clone(),
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

    // Attach async-twin pairs (foo ↔ afoo / async_foo) on each function fact.
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

    // Pass 2: collect call edges with type resolution.
    // Labels for task entries: functions referenced from create_task etc.
    let mut task_targets = TaskTargets::default();
    let mut follow_targets: HashMap<String, FollowTarget> = HashMap::new();

    for (_sys, file, program_file, rel, module) in &opened {
        let source = source_text(db, *file);
        let index = line_index(db, *file);
        let parsed = parsed_module(db, program_file.python_file(db)).load(db);
        let model = SemanticModel::new(db, *program_file);
        let ctx = FileCtx {
            db,
            program_file: *program_file,
            file: *file,
            rel: rel.clone(),
            module: module.clone(),
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

    // Apply task labels discovered from create_task / gather / etc.
    for fid in task_targets.0 {
        let f = &mut facts.functions[fid.index()];
        if f.label == EntryLabel::Other {
            f.label = EntryLabel::Task;
        }
    }

    // M2: follow reachable third-party / outside-editable `.py` callees.
    follow_third_party(
        db,
        &mut facts,
        &mut def_map,
        &mut qual_map,
        &mut follow_targets,
        &mut package_meta,
        &followable_extra_roots,
        cache_dir,
        follow_budget,
    );

    // Stable unresolved-import order (cold == warm JSON).
    facts
        .unresolved_imports
        .sort_by(|a, b| (&a.file, &a.module, a.line).cmp(&(&b.file, &b.module, b.line)));
    facts.unresolved_third_party_modules.sort();
    facts.unresolved_third_party_modules.dedup();

    // Stable edge order so cold extract vs warm cache materialization agree.
    // Never key on raw FunctionId — those differ between cold follow and warm
    // materialization even when the semantic graph is identical.
    {
        let callee_key = |e: &CallEdge| -> String {
            match &e.callee {
                CalleeRef::External { qualname } => format!("E:{qualname}"),
                CalleeRef::Unresolved { reason } => format!("U:{reason}"),
                CalleeRef::Offload(k) => format!("O:{}", k.as_str()),
                CalleeRef::ParamInvoke { index } => format!("I:{index}"),
                CalleeRef::Project(id) => {
                    let f = &facts.functions[id.index()];
                    format!("P:{}:{}", f.module, f.qualname)
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

    Ok(facts)
}
