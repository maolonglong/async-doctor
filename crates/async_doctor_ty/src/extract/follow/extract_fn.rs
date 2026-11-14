//! Single-parse per-file extraction for third-party follow targets.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;

use async_doctor_core::{EntryLabel, FunctionFact, FunctionId, ProjectFacts};
use ruff_db::files::system_path_to_file;
use ruff_db::parsed::parsed_module;
use ruff_db::source::{line_index, source_text};
use ruff_python_ast::{self as ast, Stmt};
use ruff_source_file::OneIndexed;
use ty_python_semantic::SemanticModel;

use crate::cache::third_party_module_path;
use crate::db::AsyncDoctorDb;
use crate::sitepkgs::{FollowTarget, PackageMetaMemo};

use super::super::collector::CallCollector;
use super::super::constants::{body_has_yield, collect_module_constants};
use super::super::ctx::{DefKey, FileCtx, TaskTargets};
use super::super::path::{module_name_from_relative, relative_display};

#[cfg(test)]
thread_local! {
    static THIRD_PARTY_FILE_PARSES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
pub(crate) fn reset_third_party_file_parse_count() {
    THIRD_PARTY_FILE_PARSES.with(|c| c.set(0));
}

#[cfg(test)]
pub(crate) fn third_party_file_parse_count() -> usize {
    THIRD_PARTY_FILE_PARSES.with(|c| c.get())
}

pub(super) struct ExtractedGroupFn {
    pub qualname: String,
    pub target: FollowTarget,
    pub id: FunctionId,
    /// `Some((start, end))` when extracted in this group; `None` if `id` was
    /// already extracted in an earlier wave (alias reuse).
    pub edge_range: Option<(usize, usize)>,
    /// True when this function body was newly extracted (charges follow budget).
    pub newly_extracted: bool,
}

/// Parse one third-party source file once, index its semantic model & module
/// helpers once, visit all requested `targets` in that file, and clear the AST once.
#[allow(clippy::too_many_arguments)]
pub(super) fn extract_third_party_file_group(
    db: &AsyncDoctorDb,
    facts: &mut ProjectFacts,
    def_map: &mut HashMap<DefKey, FunctionId>,
    qual_map: &mut HashMap<(String, String), FunctionId>,
    qual_to_id: &mut BTreeMap<String, FunctionId>,
    id_to_external_key: &mut HashMap<FunctionId, String>,
    follow_targets: &mut HashMap<String, FollowTarget>,
    package_meta: &mut PackageMetaMemo,
    followable_extra_roots: &[PathBuf],
    targets: &[(String, FollowTarget)],
) -> Vec<ExtractedGroupFn> {
    let Some((_, first_target)) = targets.first() else {
        return Vec::new();
    };
    let Ok(file) = system_path_to_file(db, &first_target.path) else {
        return Vec::new();
    };
    let program = db.program();
    let program_file = program.program_file(db, file);
    #[cfg(test)]
    THIRD_PARTY_FILE_PARSES.with(|c| c.set(c.get() + 1));
    let parsed = parsed_module(db, program_file.python_file(db)).load(db);
    if parsed.has_invalid_syntax() {
        parsed.module().clear();
        return Vec::new();
    }

    let source = source_text(db, file);
    let index = line_index(db, file);
    let (module, rel) = third_party_module_and_rel(&first_target.path, followable_extra_roots);
    let project_rel = relative_display(db.project_root(), &first_target.path);

    let mut fn_by_start: HashMap<u32, &ast::StmtFunctionDef> = HashMap::new();
    index_functions_by_start(parsed.suite(), &mut fn_by_start);

    let mut planned: Vec<(
        String,
        FollowTarget,
        FunctionId,
        Option<&ast::StmtFunctionDef>,
    )> = Vec::with_capacity(targets.len());
    let mut any_new_body = false;

    for (ext_qual, target) in targets {
        let Some(&func) = fn_by_start.get(&target.name_start) else {
            continue;
        };
        let key = DefKey {
            file: rel.clone(),
            name_start: target.name_start,
        };
        let proj_key = DefKey {
            file: project_rel.clone(),
            name_start: target.name_start,
        };
        if let Some(existing_id) = def_map
            .get(&key)
            .copied()
            .or_else(|| def_map.get(&proj_key).copied())
        {
            def_map.insert(key, existing_id);
            def_map.insert(proj_key, existing_id);
            qual_to_id.insert(ext_qual.clone(), existing_id);
            id_to_external_key
                .entry(existing_id)
                .or_insert_with(|| ext_qual.clone());
            planned.push((ext_qual.clone(), target.clone(), existing_id, None));
            continue;
        }

        let local_qual = target
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
        let line: OneIndexed = index.line_index(func.name.range.start());
        let end_line: OneIndexed = index.line_index(func.range.end());
        facts.functions.push(FunctionFact {
            id,
            file: rel.clone(),
            line: line.get() as u32,
            end_line: end_line.get() as u32,
            module: module.clone(),
            qualname: local_qual.clone(),
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
            offload_decorator: None,
        });
        def_map.insert(key, id);
        def_map.insert(proj_key, id);
        qual_map.insert((module.clone(), local_qual), id);
        qual_to_id.insert(ext_qual.clone(), id);
        id_to_external_key.insert(id, ext_qual.clone());
        planned.push((ext_qual.clone(), target.clone(), id, Some(func)));
        any_new_body = true;
    }

    let mut out = Vec::with_capacity(planned.len());
    if any_new_body {
        let ctx = FileCtx {
            db,
            program_file,
            file,
            rel,
            module,
            source,
            index,
        };
        let model = SemanticModel::new(db, program_file);
        let module_constants = collect_module_constants(parsed.suite());
        let mut idle_loop_helpers: HashSet<String> = HashSet::new();
        super::super::collector::loop_guards::collect_idle_loop_helpers(
            parsed.suite(),
            &mut idle_loop_helpers,
        );
        let property_defs: HashSet<FunctionId> = HashSet::new();
        let mut visited_ranges: HashMap<FunctionId, (usize, usize)> = HashMap::new();

        for (ext_qual, target, id, maybe_func) in planned {
            if let Some(func) = maybe_func {
                let edge_start = facts.edges.len();
                let param_names_for_visit = facts.functions[id.index()].param_names.clone();
                let mut task_targets = TaskTargets::default();
                let mut visitor = CallCollector::new(
                    &ctx,
                    &model,
                    facts,
                    def_map,
                    None,
                    qual_map,
                    None,
                    &property_defs,
                    &mut task_targets,
                    follow_targets,
                    package_meta,
                    followable_extra_roots,
                    false,
                    &module_constants,
                );
                visitor.idle_loop_helpers.clone_from(&idle_loop_helpers);
                visitor.current_fn = Some(id);
                visitor.current_param_names = param_names_for_visit;
                visitor.note_decorator_refs(id, func);
                visitor.visit_body_seq(&func.body);
                let edge_end = facts.edges.len();
                visited_ranges.insert(id, (edge_start, edge_end));
                out.push(ExtractedGroupFn {
                    qualname: ext_qual,
                    target,
                    id,
                    edge_range: Some((edge_start, edge_end)),
                    newly_extracted: true,
                });
            } else {
                out.push(ExtractedGroupFn {
                    qualname: ext_qual,
                    target,
                    id,
                    edge_range: visited_ranges.get(&id).copied(),
                    newly_extracted: false,
                });
            }
        }
    } else {
        for (ext_qual, target, id, _) in planned {
            out.push(ExtractedGroupFn {
                qualname: ext_qual,
                target,
                id,
                edge_range: None,
                newly_extracted: false,
            });
        }
    }

    parsed.module().clear();
    out
}

fn third_party_module_and_rel(
    target_path: &ruff_db::system::SystemPath,
    followable_extra_roots: &[PathBuf],
) -> (String, String) {
    let abs = target_path.as_str().replace('\\', "/");
    let editable_rel = followable_extra_roots.iter().find_map(|r| {
        let rs = r.to_string_lossy().replace('\\', "/");
        let prefix = if rs.ends_with('/') {
            rs
        } else {
            format!("{rs}/")
        };
        abs.strip_prefix(&prefix).map(str::to_string)
    });
    let module = third_party_module_path(&abs)
        .map(module_name_from_relative)
        .or_else(|| editable_rel.as_deref().map(module_name_from_relative))
        .unwrap_or_default();
    let tree = if abs.contains("/dist-packages/") {
        "dist-packages"
    } else if editable_rel.is_some() {
        "editable"
    } else {
        "site-packages"
    };
    let rel = if let Some(rest) = third_party_module_path(&abs) {
        format!("{tree}/{rest}")
    } else if let Some(rest) = &editable_rel {
        format!("{tree}/{rest}")
    } else if !module.is_empty() {
        format!("{tree}/{}.py", module.replace('.', "/"))
    } else {
        abs
    };
    (module, rel)
}

fn index_functions_by_start<'a>(
    body: &'a [Stmt],
    out: &mut HashMap<u32, &'a ast::StmtFunctionDef>,
) {
    for stmt in body {
        match stmt {
            Stmt::FunctionDef(f) => {
                out.entry(u32::from(f.name.range.start())).or_insert(f);
                index_functions_by_start(&f.body, out);
            }
            Stmt::ClassDef(c) => index_functions_by_start(&c.body, out),
            Stmt::If(i) => {
                index_functions_by_start(&i.body, out);
                for c in &i.elif_else_clauses {
                    index_functions_by_start(&c.body, out);
                }
            }
            Stmt::For(f) => {
                index_functions_by_start(&f.body, out);
                index_functions_by_start(&f.orelse, out);
            }
            Stmt::While(w) => {
                index_functions_by_start(&w.body, out);
                index_functions_by_start(&w.orelse, out);
            }
            Stmt::With(w) => index_functions_by_start(&w.body, out),
            Stmt::Try(t) => {
                index_functions_by_start(&t.body, out);
                for h in &t.handlers {
                    let ast::ExceptHandler::ExceptHandler(eh) = h;
                    index_functions_by_start(&eh.body, out);
                }
                index_functions_by_start(&t.orelse, out);
                index_functions_by_start(&t.finalbody, out);
            }
            Stmt::Match(m) => {
                for c in &m.cases {
                    index_functions_by_start(&c.body, out);
                }
            }
            _ => {}
        }
    }
}
