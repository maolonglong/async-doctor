//! First-party function discovery and entry-label inference.

use std::collections::{HashMap, HashSet};

use async_doctor_core::{EntryLabel, FunctionFact, FunctionId, ProjectFacts};
use ruff_python_ast::{self as ast, Expr, Stmt};
use ruff_source_file::{LineIndex, OneIndexed};

use super::constants::body_has_yield;
use super::ctx::{DefKey, scope_qualname};

pub(crate) struct Pass1Fn {
    pub(crate) name_start: u32,
    pub(crate) line: u32,
    pub(crate) qualname: String,
    pub(crate) is_async: bool,
    pub(crate) is_generator: bool,
    pub(crate) label: EntryLabel,
    pub(crate) is_sync_route: bool,
    pub(crate) param_names: Vec<String>,
}

pub(crate) struct Pass1FileData {
    pub(crate) functions: Vec<Pass1Fn>,
    pub(crate) class_methods: Vec<(String, String)>,
    pub(crate) property_name_starts: Vec<u32>,
}

pub(crate) enum Pass1FileOutcome {
    Skipped(String),
    Ok(Pass1FileData),
}

pub(crate) fn collect_functions_standalone(index: &LineIndex, suite: &[Stmt]) -> Pass1FileData {
    struct Collector<'a> {
        index: &'a LineIndex,
        data: Pass1FileData,
        class_stack: Vec<String>,
        fn_stack: Vec<String>,
    }

    impl Collector<'_> {
        fn visit_body(&mut self, body: &[Stmt]) {
            for stmt in body {
                self.visit_stmt(stmt);
            }
        }

        fn visit_stmt(&mut self, stmt: &Stmt) {
            match stmt {
                Stmt::ClassDef(class) => {
                    self.class_stack.push(class.name.as_str().to_string());
                    self.visit_body(&class.body);
                    self.class_stack.pop();
                }
                Stmt::FunctionDef(func) => {
                    let qualname =
                        scope_qualname(&self.class_stack, &self.fn_stack, func.name.as_str());
                    if !self.class_stack.is_empty() && self.fn_stack.is_empty() {
                        let class = self.class_stack.join(".");
                        self.data
                            .class_methods
                            .push((class, func.name.as_str().to_string()));
                    }

                    let label = infer_entry_label(func);
                    let is_sync_route = !func.is_async && label == EntryLabel::Route;
                    let name_start = u32::from(func.name.range.start());
                    if func
                        .decorator_list
                        .iter()
                        .any(|d| decorator_is_property(&d.expression))
                    {
                        self.data.property_name_starts.push(name_start);
                    }

                    let param_names: Vec<String> = func
                        .parameters
                        .iter_non_variadic_params()
                        .map(|a| a.parameter.name.as_str().to_string())
                        .collect();
                    let line: OneIndexed = self.index.line_index(func.name.range.start());
                    self.data.functions.push(Pass1Fn {
                        name_start,
                        line: line.get() as u32,
                        qualname,
                        is_async: func.is_async,
                        is_generator: body_has_yield(&func.body),
                        label,
                        is_sync_route,
                        param_names,
                    });

                    self.fn_stack.push(func.name.as_str().to_string());
                    self.visit_body(&func.body);
                    self.fn_stack.pop();
                }
                Stmt::If(if_stmt) => {
                    self.visit_body(&if_stmt.body);
                    for clause in &if_stmt.elif_else_clauses {
                        self.visit_body(&clause.body);
                    }
                }
                Stmt::For(for_stmt) => {
                    self.visit_body(&for_stmt.body);
                    self.visit_body(&for_stmt.orelse);
                }
                Stmt::While(while_stmt) => {
                    self.visit_body(&while_stmt.body);
                    self.visit_body(&while_stmt.orelse);
                }
                Stmt::With(with_stmt) => {
                    self.visit_body(&with_stmt.body);
                }
                Stmt::Try(try_stmt) => {
                    self.visit_body(&try_stmt.body);
                    for handler in &try_stmt.handlers {
                        let ast::ExceptHandler::ExceptHandler(h) = handler;
                        self.visit_body(&h.body);
                    }
                    self.visit_body(&try_stmt.orelse);
                    self.visit_body(&try_stmt.finalbody);
                }
                Stmt::Match(match_stmt) => {
                    for case in &match_stmt.cases {
                        self.visit_body(&case.body);
                    }
                }
                _ => {}
            }
        }
    }

    let mut c = Collector {
        index,
        data: Pass1FileData {
            functions: Vec::new(),
            class_methods: Vec::new(),
            property_name_starts: Vec::new(),
        },
        class_stack: Vec::new(),
        fn_stack: Vec::new(),
    };
    c.visit_body(suite);
    c.data
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn merge_pass1_file_data(
    rel: &str,
    module: &str,
    data: Pass1FileData,
    facts: &mut ProjectFacts,
    def_map: &mut HashMap<DefKey, FunctionId>,
    qual_map: &mut HashMap<(String, String), FunctionId>,
    class_methods: &mut HashMap<(String, String), Vec<String>>,
    property_defs: &mut HashSet<DefKey>,
) {
    for (class, method) in data.class_methods {
        class_methods
            .entry((module.to_string(), class))
            .or_default()
            .push(method);
    }
    for name_start in data.property_name_starts {
        property_defs.insert(DefKey {
            file: rel.to_string(),
            name_start,
        });
    }
    for f in data.functions {
        let id = FunctionId(facts.functions.len() as u32);
        let key = DefKey {
            file: rel.to_string(),
            name_start: f.name_start,
        };
        def_map.insert(key, id);
        qual_map.insert((module.to_string(), f.qualname.clone()), id);
        facts.functions.push(FunctionFact {
            id,
            file: rel.to_string(),
            line: f.line,
            module: module.to_string(),
            qualname: f.qualname,
            is_async: f.is_async,
            is_generator: f.is_generator,
            label: f.label,
            is_sync_route: f.is_sync_route,
            async_twins: Vec::new(),
            param_names: f.param_names,
            is_third_party: false,
            package_name: None,
            package_version: None,
            invokes_params: Vec::new(),
            is_offload_wrapper: false,
            offload_decorator: None,
        });
    }
}

pub(crate) fn twin_name(method: &str, methods: &[String]) -> Option<String> {
    let candidates = [
        format!("a{method}"),
        format!("async_{method}"),
        format!("{method}_async"),
    ];
    candidates
        .into_iter()
        .find(|c| methods.iter().any(|m| m == c))
}

pub(crate) fn decorator_is_property(expr: &Expr) -> bool {
    match expr {
        Expr::Call(call) => decorator_is_property(&call.func),
        Expr::Name(n) => matches!(n.id.as_str(), "property" | "cached_property"),
        Expr::Attribute(a) => matches!(a.attr.as_str(), "property" | "cached_property"),
        _ => false,
    }
}

pub(crate) fn infer_entry_label(func: &ast::StmtFunctionDef) -> EntryLabel {
    let name = func.name.as_str();
    if matches!(
        name,
        "lifespan" | "startup" | "shutdown" | "on_startup" | "on_shutdown"
    ) {
        return EntryLabel::Lifecycle;
    }

    for deco in &func.decorator_list {
        if decorator_is_route(&deco.expression) {
            return EntryLabel::Route;
        }
        if decorator_is_lifecycle(&deco.expression) {
            return EntryLabel::Lifecycle;
        }
    }
    EntryLabel::Other
}

pub(crate) fn decorator_is_route(expr: &Expr) -> bool {
    match expr {
        Expr::Call(call) => decorator_is_route(&call.func),
        Expr::Attribute(attr) => {
            matches!(
                attr.attr.as_str(),
                "get"
                    | "post"
                    | "put"
                    | "delete"
                    | "patch"
                    | "head"
                    | "options"
                    | "route"
                    | "api_route"
                    | "websocket"
                    | "websocket_route"
                    | "on_event"
            )
        }
        Expr::Name(name) => matches!(
            name.id.as_str(),
            "websocket" | "route" | "get" | "post" | "put" | "delete" | "patch"
        ),
        _ => false,
    }
}

pub(crate) fn decorator_is_lifecycle(expr: &Expr) -> bool {
    match expr {
        Expr::Call(call) => {
            if decorator_is_lifecycle(&call.func) {
                return true;
            }
            // @app.on_event("startup")
            if let Expr::Attribute(attr) = call.func.as_ref()
                && attr.attr.as_str() == "on_event"
            {
                return true;
            }
            false
        }
        Expr::Attribute(attr) => matches!(attr.attr.as_str(), "on_event" | "lifespan"),
        Expr::Name(name) => matches!(name.id.as_str(), "lifespan"),
        _ => false,
    }
}
