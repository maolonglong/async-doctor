//! Extract first-party call-graph facts using ty's semantic model.
//!
//! All ty/ruff usage stays in this crate. Core sees only plain [`ProjectFacts`].

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::Context;
use async_doctor_core::{
    CallEdge, CallKind, CalleeRef, ConditionTag, EntryLabel, FunctionFact, FunctionId, LiteralArg,
    OffloadKind, ParamGuard, ProjectFacts, SinkTable,
};
use ignore::WalkBuilder;
use ruff_db::files::{File, FilePath, system_path_to_file};
use ruff_db::parsed::parsed_module;
use ruff_db::source::{line_index, source_text};
use ruff_db::system::{SystemPath, SystemPathBuf};
use ruff_python_ast::visitor::{Visitor, walk_expr, walk_stmt};
use ruff_python_ast::{self as ast, Expr, Stmt};
use ruff_source_file::OneIndexed;
use ruff_text_size::{Ranged, TextRange, TextSize};
use ty_python_core::definition::DefinitionKind;
use ty_python_core::{ProgramFile, semantic_index};
use ty_python_semantic::types::{Type, TypeDefinition};
use ty_python_semantic::{
    HasType, ImportAliasResolution, SemanticModel, definitions_for_attribute, definitions_for_name,
};

use crate::cache::{effective_cache_root, third_party_module_path};
use crate::db::AsyncDoctorDb;
use crate::sitepkgs::{
    FollowTarget, PackageCacheBatch, clear_package_meta_memo, is_followable_site_packages_py,
    materialize_cached_functions, package_meta_for_path,
};
use async_doctor_rules::{BuiltinSinkTable, is_async_counterpart};

/// Scan a project directory or a single `.py` file into graded facts.
pub fn extract_path(path: &Path) -> anyhow::Result<ProjectFacts> {
    extract_path_with_cache(path, None)
}

/// Extract call-graph facts, writing/reading the third-party package cache under
/// `cache_dir` when provided (otherwise the user default cache root).
pub fn extract_path_with_cache(
    path: &Path,
    cache_dir: Option<&Path>,
) -> anyhow::Result<ProjectFacts> {
    clear_package_meta_memo();
    let path = absolute_utf8(path)?;
    if path.as_std_path().is_file() {
        if path.extension() != Some("py") {
            anyhow::bail!("`{path}` is not a Python (`.py`) file");
        }
        let parent = path
            .parent()
            .map(SystemPath::to_path_buf)
            .unwrap_or_else(|| SystemPathBuf::from("."));
        let db = AsyncDoctorDb::open(&parent, None)?;
        return extract_files(&db, vec![path.as_std_path().to_path_buf()], cache_dir);
    }

    let db = AsyncDoctorDb::open(&path, None)?;
    let files = discover_python_files(db.project_root().as_std_path())?;
    extract_files(&db, files, cache_dir)
}

fn absolute_utf8(path: &Path) -> anyhow::Result<SystemPathBuf> {
    let path = std::path::absolute(path)
        .with_context(|| format!("cannot resolve path `{}`", path.display()))?;
    SystemPathBuf::from_path_buf(path)
        .map_err(|p| anyhow::anyhow!("`{}` is not valid UTF-8", p.display()))
}

fn discover_python_files(root: &Path) -> anyhow::Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    let walker = WalkBuilder::new(root)
        .hidden(false)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .build();

    for entry in walker {
        let entry = entry.context("directory walk failed")?;
        if !entry.file_type().is_some_and(|ft| ft.is_file()) {
            continue;
        }
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("py") {
            continue;
        }
        if path.components().any(|c| {
            matches!(
                c.as_os_str().to_str(),
                Some(".venv" | "venv" | "site-packages" | "__pycache__" | ".git" | "target")
            )
        }) {
            continue;
        }
        paths.push(path.to_path_buf());
    }
    paths.sort();
    Ok(paths)
}

fn relative_display(root: &SystemPath, path: &SystemPath) -> String {
    path.strip_prefix(root)
        .map(|p| p.as_str().replace('\\', "/"))
        .unwrap_or_else(|_| path.as_str().replace('\\', "/"))
}

fn module_name_from_relative(rel: &str) -> String {
    let mut path = rel.replace('\\', "/");
    if let Some(stripped) = path.strip_suffix(".py") {
        path = stripped.to_string();
    }
    if let Some(stripped) = path.strip_suffix("/__init__") {
        path = stripped.to_string();
    }
    if path == "__init__" || path.is_empty() {
        return String::new();
    }
    path.replace('/', ".")
}

/// Key for looking up a project function by definition site.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct DefKey {
    file: String,
    /// Start offset of the function name.
    name_start: u32,
}

struct FileCtx<'db> {
    db: &'db AsyncDoctorDb,
    #[allow(dead_code)]
    program_file: ProgramFile<'db>,
    #[allow(dead_code)]
    file: File,
    rel: String,
    module: String,
    source: String,
    index: ruff_source_file::LineIndex,
}

impl<'db> FileCtx<'db> {
    fn line_of(&self, offset: TextSize) -> u32 {
        let line: OneIndexed = self.index.line_index(offset);
        line.get() as u32
    }

    fn snippet(&self, range: TextRange) -> String {
        let start = usize::from(range.start());
        let end = usize::from(range.end()).min(self.source.len());
        let start = start.min(end);
        self.source[start..end].trim().to_string()
    }
}

fn extract_files(
    db: &AsyncDoctorDb,
    files: Vec<PathBuf>,
    cache_dir: Option<&Path>,
) -> anyhow::Result<ProjectFacts> {
    let mut facts = ProjectFacts {
        complete: true,
        ..ProjectFacts::default()
    };
    let program = db.program();

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
            async_with_receivers: HashMap::new(),
            occurrence_counter: HashMap::new(),
            pending_literals: Vec::new(),
            pending_callables: Vec::new(),
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

    // M2: follow reachable third-party `.py` callees under site-packages.
    follow_third_party(
        db,
        &mut facts,
        &mut def_map,
        &mut qual_map,
        &mut follow_targets,
        cache_dir,
    );

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

fn twin_name(method: &str, methods: &[String]) -> Option<String> {
    let candidates = [
        format!("a{method}"),
        format!("async_{method}"),
        format!("{method}_async"),
    ];
    candidates
        .into_iter()
        .find(|c| methods.iter().any(|m| m == c))
}

#[derive(Default)]
struct TaskTargets(std::collections::HashSet<FunctionId>);

fn scope_qualname(class_stack: &[String], fn_stack: &[String], name: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for c in class_stack {
        parts.push(c.as_str());
    }
    for f in fn_stack {
        parts.push(f.as_str());
    }
    parts.push(name);
    parts.join(".")
}

fn decorator_is_property(expr: &Expr) -> bool {
    match expr {
        Expr::Call(call) => decorator_is_property(&call.func),
        Expr::Name(n) => n.id.as_str() == "property",
        Expr::Attribute(a) => a.attr.as_str() == "property",
        _ => false,
    }
}

fn collect_functions(
    ctx: &FileCtx<'_>,
    suite: &[Stmt],
    facts: &mut ProjectFacts,
    def_map: &mut HashMap<DefKey, FunctionId>,
    qual_map: &mut HashMap<(String, String), FunctionId>,
    class_methods: &mut HashMap<(String, String), Vec<String>>,
    property_defs: &mut HashSet<DefKey>,
) {
    struct Collector<'a, 'db> {
        ctx: &'a FileCtx<'db>,
        facts: &'a mut ProjectFacts,
        def_map: &'a mut HashMap<DefKey, FunctionId>,
        qual_map: &'a mut HashMap<(String, String), FunctionId>,
        class_methods: &'a mut HashMap<(String, String), Vec<String>>,
        property_defs: &'a mut HashSet<DefKey>,
        class_stack: Vec<String>,
        fn_stack: Vec<String>,
    }

    impl Collector<'_, '_> {
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
                        self.class_methods
                            .entry((self.ctx.module.clone(), class))
                            .or_default()
                            .push(func.name.as_str().to_string());
                    }

                    let label = infer_entry_label(func);
                    let is_sync_route = !func.is_async && label == EntryLabel::Route;
                    let id = FunctionId(self.facts.functions.len() as u32);
                    let key = DefKey {
                        file: self.ctx.rel.clone(),
                        name_start: u32::from(func.name.range.start()),
                    };
                    if func
                        .decorator_list
                        .iter()
                        .any(|d| decorator_is_property(&d.expression))
                    {
                        self.property_defs.insert(key.clone());
                    }
                    self.def_map.insert(key, id);
                    self.qual_map
                        .insert((self.ctx.module.clone(), qualname.clone()), id);

                    let param_names: Vec<String> = func
                        .parameters
                        .iter_non_variadic_params()
                        .map(|a| a.parameter.name.as_str().to_string())
                        .collect();
                    self.facts.functions.push(FunctionFact {
                        id,
                        file: self.ctx.rel.clone(),
                        line: self.ctx.line_of(func.name.range.start()),
                        module: self.ctx.module.clone(),
                        qualname,
                        is_async: func.is_async,
                        is_generator: body_has_yield(&func.body),
                        label,
                        is_sync_route,
                        async_twins: Vec::new(),
                        param_names,
                        is_third_party: false,
                        package_name: None,
                        package_version: None,
                        invokes_params: Vec::new(),
                        is_offload_wrapper: false,
                    });

                    // Nested functions — push enclosing name onto fn_stack.
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
        ctx,
        facts,
        def_map,
        qual_map,
        class_methods,
        property_defs,
        class_stack: Vec::new(),
        fn_stack: Vec::new(),
    };
    c.visit_body(suite);
}

fn infer_entry_label(func: &ast::StmtFunctionDef) -> EntryLabel {
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

fn decorator_is_route(expr: &Expr) -> bool {
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

fn decorator_is_lifecycle(expr: &Expr) -> bool {
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

/// Module-level constant value we can prove (assigned once, never reassigned).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ModuleConst {
    True,
    False,
    None,
    /// Empty tuple/list/dict/set/string — falsy.
    Empty,
}

fn body_has_yield(body: &[Stmt]) -> bool {
    struct YieldFinder {
        found: bool,
    }
    impl<'a> Visitor<'a> for YieldFinder {
        fn visit_expr(&mut self, expr: &'a Expr) {
            if self.found {
                return;
            }
            match expr {
                Expr::Yield(_) | Expr::YieldFrom(_) => {
                    self.found = true;
                }
                _ => walk_expr(self, expr),
            }
        }
        fn visit_stmt(&mut self, stmt: &'a Stmt) {
            if self.found {
                return;
            }
            // Nested function defs have their own generator status.
            if matches!(stmt, Stmt::FunctionDef(_)) {
                return;
            }
            walk_stmt(self, stmt);
        }
    }
    let mut v = YieldFinder { found: false };
    for s in body {
        v.visit_stmt(s);
        if v.found {
            return true;
        }
    }
    false
}

fn const_value(expr: &Expr) -> Option<ModuleConst> {
    match expr {
        Expr::BooleanLiteral(b) => Some(if b.value {
            ModuleConst::True
        } else {
            ModuleConst::False
        }),
        Expr::NoneLiteral(_) => Some(ModuleConst::None),
        Expr::Tuple(t) if t.elts.is_empty() => Some(ModuleConst::Empty),
        Expr::List(l) if l.elts.is_empty() => Some(ModuleConst::Empty),
        Expr::Dict(d) if d.items.is_empty() => Some(ModuleConst::Empty),
        Expr::Set(s) if s.elts.is_empty() => Some(ModuleConst::Empty),
        Expr::StringLiteral(s) if s.value.to_str().is_empty() => Some(ModuleConst::Empty),
        Expr::Call(call)
            if matches!(
                call.func.as_ref(),
                Expr::Name(n) if matches!(n.id.as_str(), "dict" | "list" | "set" | "tuple")
            ) && call.arguments.args.is_empty()
                && call.arguments.keywords.is_empty() =>
        {
            Some(ModuleConst::Empty)
        }
        _ => None,
    }
}

/// Names that appear in any `global NAME` in this module — config toggles; never prune.
fn collect_names_declared_global(suite: &[Stmt]) -> HashSet<String> {
    let mut out = HashSet::new();
    fn walk(stmts: &[Stmt], out: &mut HashSet<String>) {
        for stmt in stmts {
            match stmt {
                Stmt::Global(g) => {
                    for n in &g.names {
                        out.insert(n.as_str().to_string());
                    }
                }
                Stmt::FunctionDef(f) => walk(&f.body, out),
                Stmt::ClassDef(c) => walk(&c.body, out),
                Stmt::If(i) => {
                    walk(&i.body, out);
                    for c in &i.elif_else_clauses {
                        walk(&c.body, out);
                    }
                }
                Stmt::For(f) => {
                    walk(&f.body, out);
                    walk(&f.orelse, out);
                }
                Stmt::While(w) => {
                    walk(&w.body, out);
                    walk(&w.orelse, out);
                }
                Stmt::With(w) => walk(&w.body, out),
                Stmt::Try(t) => {
                    walk(&t.body, out);
                    for h in &t.handlers {
                        let ast::ExceptHandler::ExceptHandler(eh) = h;
                        walk(&eh.body, out);
                    }
                    walk(&t.orelse, out);
                    walk(&t.finalbody, out);
                }
                Stmt::Match(m) => {
                    for c in &m.cases {
                        walk(&c.body, out);
                    }
                }
                _ => {}
            }
        }
    }
    walk(suite, &mut out);
    out
}

fn collect_module_constants(suite: &[Stmt]) -> HashMap<String, ModuleConst> {
    let mut assign_count: HashMap<String, u32> = HashMap::new();
    let mut values: HashMap<String, ModuleConst> = HashMap::new();
    let globals = collect_names_declared_global(suite);

    let bump = |counts: &mut HashMap<String, u32>, name: &str| {
        *counts.entry(name.to_string()).or_insert(0) += 1;
    };

    for stmt in suite {
        match stmt {
            Stmt::Assign(a) => {
                if a.targets.len() != 1 {
                    continue;
                }
                let Expr::Name(n) = &a.targets[0] else {
                    continue;
                };
                let name = n.id.as_str();
                bump(&mut assign_count, name);
                if let Some(v) = const_value(&a.value) {
                    values.insert(name.to_string(), v);
                } else {
                    values.remove(name);
                }
            }
            Stmt::AnnAssign(a) => {
                let Expr::Name(n) = a.target.as_ref() else {
                    continue;
                };
                let name = n.id.as_str();
                bump(&mut assign_count, name);
                if let Some(val) = &a.value {
                    if let Some(v) = const_value(val) {
                        values.insert(name.to_string(), v);
                    } else {
                        values.remove(name);
                    }
                }
            }
            Stmt::AugAssign(a) => {
                if let Expr::Name(n) = a.target.as_ref() {
                    bump(&mut assign_count, n.id.as_str());
                    // Force count >= 2 so we drop it as unstable.
                    bump(&mut assign_count, n.id.as_str());
                    values.remove(n.id.as_str());
                }
            }
            _ => {}
        }
    }

    values.retain(|k, _| assign_count.get(k).copied().unwrap_or(0) == 1 && !globals.contains(k));
    values
}

fn literal_arg(expr: &Expr) -> Option<LiteralArg> {
    match expr {
        Expr::BooleanLiteral(b) => Some(if b.value {
            LiteralArg::True
        } else {
            LiteralArg::False
        }),
        Expr::NoneLiteral(_) => Some(LiteralArg::None),
        _ => None,
    }
}

fn is_generator_consumer(func: &Expr) -> bool {
    match func {
        Expr::Name(n) => matches!(
            n.id.as_str(),
            "list" | "tuple" | "set" | "next" | "sorted" | "frozenset"
        ),
        Expr::Attribute(a) => matches!(
            a.attr.as_str(),
            "list" | "tuple" | "set" | "next" | "sorted" | "frozenset"
        ),
        _ => false,
    }
}

/// Detect `if flag:` / `if not flag:` where `flag` is a positional parameter.
fn param_guard_from_test(test: &Expr, param_names: &[String]) -> Option<ParamGuard> {
    let (name, inverted) = match test {
        Expr::Name(n) => (n.id.as_str(), false),
        Expr::UnaryOp(u) if matches!(u.op, ast::UnaryOp::Not) => match u.operand.as_ref() {
            Expr::Name(n) => (n.id.as_str(), true),
            _ => return None,
        },
        _ => return None,
    };
    param_names
        .iter()
        .position(|p| p == name)
        .map(|index| ParamGuard {
            index: index as u32,
            inverted,
        })
}

/// Cheap cache-miss tests: `x is None`, `k not in cache`.
fn is_cache_miss_test(test: &Expr) -> bool {
    match test {
        Expr::Compare(cmp) if cmp.ops.len() == 1 => {
            let op = cmp.ops[0];
            match op {
                ast::CmpOp::Is => {
                    // `cache is None`
                    matches!(cmp.operands.get(1), Some(Expr::NoneLiteral(_)))
                        && matches!(cmp.operands.first(), Some(Expr::Name(_)))
                }
                ast::CmpOp::NotIn => {
                    // `key not in cache`
                    matches!(cmp.operands.get(1), Some(Expr::Name(_)))
                }
                _ => false,
            }
        }
        _ => false,
    }
}

/// Cache-*hit* early-exit tests: `x is not None`, `k in cache`.
fn is_cache_hit_test(test: &Expr) -> bool {
    match test {
        Expr::Compare(cmp) if cmp.ops.len() == 1 => match cmp.ops[0] {
            ast::CmpOp::IsNot => {
                matches!(cmp.operands.get(1), Some(Expr::NoneLiteral(_)))
                    && matches!(cmp.operands.first(), Some(Expr::Name(_)))
            }
            ast::CmpOp::In => matches!(cmp.operands.get(1), Some(Expr::Name(_))),
            _ => false,
        },
        _ => false,
    }
}

fn body_always_exits(body: &[Stmt]) -> bool {
    !body.is_empty()
        && body
            .iter()
            .all(|s| matches!(s, Stmt::Return(_) | Stmt::Raise(_)))
}

#[derive(Debug, Clone, Copy)]
enum EarlyExitKind {
    /// Guard is provably always true and body exits — rest of suite is dead.
    UnreachableRest,
    /// Guard may fail — rest of suite is conditional (optionally cache-miss).
    Conditional { cache_miss: bool },
}

struct CallCollector<'a, 'db> {
    ctx: &'a FileCtx<'db>,
    model: &'a SemanticModel<'db>,
    facts: &'a mut ProjectFacts,
    def_map: &'a mut HashMap<DefKey, FunctionId>,
    qual_map: &'a mut HashMap<(String, String), FunctionId>,
    property_defs: &'a HashSet<DefKey>,
    task_targets: &'a mut TaskTargets,
    follow_targets: &'a mut HashMap<String, FollowTarget>,
    module_constants: &'a HashMap<String, ModuleConst>,
    current_fn: Option<FunctionId>,
    current_param_names: Vec<String>,
    class_stack: Vec<String>,
    fn_stack: Vec<String>,
    dead_depth: u32,
    loop_depth: u32,
    branch_depth: u32,
    cache_miss_depth: u32,
    active_param_guard: Option<ParamGuard>,
    /// True while visiting an expression that consumes a generator.
    consuming_generator: bool,
    in_lambda: bool,
    /// When true, the enclosing lambda was passed to an offload boundary.
    skip_lambda_body: bool,
    /// True while visiting `call.func` — suppress PropertyGet on that Attribute.
    in_callee_position: bool,
    /// When true, the *next* recorded Call edge is the direct operand of
    /// `await` / `async for` iter / `async with` context (nested arg calls are not).
    pending_is_awaited: bool,
    /// Locals bound by `async with <asyncio-redis> as name` → receiver type qual.
    async_with_receivers: HashMap<String, String>,
    occurrence_counter: HashMap<(FunctionId, String), u32>,
    /// Literals for the call currently being recorded (set by handle_call).
    pending_literals: Vec<Option<LiteralArg>>,
    /// Positional callables for the call currently being recorded.
    pending_callables: Vec<Option<FunctionId>>,
    /// Local name → lambda assigned in the current function.
    local_lambdas: HashMap<String, &'a ast::ExprLambda>,
    /// Local name → generator expression assigned in the current function.
    local_genexprs: HashMap<String, &'a ast::ExprGenerator>,
}

impl<'a, 'db> Visitor<'a> for CallCollector<'a, 'db> {
    fn visit_stmt(&mut self, stmt: &'a Stmt) {
        if self.skip_lambda_body {
            return;
        }
        match stmt {
            Stmt::FunctionDef(func) => {
                let qualname =
                    scope_qualname(&self.class_stack, &self.fn_stack, func.name.as_str());
                let fid = self
                    .qual_map
                    .get(&(self.ctx.module.clone(), qualname))
                    .copied();
                let prev = self.current_fn;
                let prev_params = std::mem::take(&mut self.current_param_names);
                let prev_lambdas = std::mem::take(&mut self.local_lambdas);
                let prev_genexprs = std::mem::take(&mut self.local_genexprs);
                self.current_fn = fid;
                self.current_param_names = fid
                    .map(|id| self.facts.function(id).param_names.clone())
                    .unwrap_or_default();
                self.fn_stack.push(func.name.as_str().to_string());
                self.visit_body_seq(&func.body);
                self.fn_stack.pop();
                self.current_fn = prev;
                self.current_param_names = prev_params;
                self.local_lambdas = prev_lambdas;
                self.local_genexprs = prev_genexprs;
            }
            Stmt::ClassDef(class) => {
                self.class_stack.push(class.name.as_str().to_string());
                self.visit_body_seq(&class.body);
                self.class_stack.pop();
            }
            Stmt::Assign(assign) => {
                self.note_local_assign(assign);
                walk_stmt(self, stmt);
            }
            Stmt::AnnAssign(ann) => {
                if let (Expr::Name(n), Some(value)) = (ann.target.as_ref(), ann.value.as_ref()) {
                    self.note_local_binding(n.id.as_str(), value);
                }
                walk_stmt(self, stmt);
            }
            Stmt::If(if_stmt) => self.visit_if(if_stmt),
            Stmt::For(for_stmt) => {
                self.visit_for(for_stmt);
            }
            Stmt::While(while_stmt) => {
                self.loop_depth += 1;
                self.visit_expr(&while_stmt.test);
                self.visit_body_seq(&while_stmt.body);
                self.loop_depth -= 1;
                self.visit_body_seq(&while_stmt.orelse);
            }
            Stmt::With(with_stmt) => self.visit_with(with_stmt),
            Stmt::Try(try_stmt) => self.visit_try(try_stmt),
            Stmt::Match(match_stmt) => self.visit_match(match_stmt),
            other => walk_stmt(self, other),
        }
    }

    fn visit_expr(&mut self, expr: &'a Expr) {
        if self.skip_lambda_body {
            return;
        }
        match expr {
            Expr::Lambda(lambda) => {
                // Deferred: only walk parameters unless later called / offloaded specially.
                if let Some(parameters) = &lambda.parameters {
                    self.visit_parameters(parameters);
                }
                // Do not walk lambda body by default (deferred execution).
            }
            Expr::Await(await_expr) => {
                // Only the Call that is the *direct* await operand is marked awaited;
                // nested calls in arguments (e.g. await send(open(p).read())) stay sync sinks.
                if let Expr::Call(call) = await_expr.value.as_ref() {
                    self.handle_call(call, true);
                    self.walk_call_children(call);
                } else {
                    self.visit_expr(&await_expr.value);
                }
            }
            Expr::Call(call) => {
                self.handle_call(call, false);
                self.walk_call_children(call);
            }
            Expr::Attribute(attr) => {
                if !self.in_callee_position {
                    self.maybe_property_get(attr);
                }
                walk_expr(self, expr);
            }
            Expr::Name(name) => {
                // Consuming a locally-assigned genexpr by name.
                if self.consuming_generator
                    && let Some(gexpr) = self.local_genexprs.get(name.id.as_str()).copied()
                {
                    self.visit_comprehension_like(&gexpr.elt, None, &gexpr.generators);
                }
                walk_expr(self, expr);
            }
            Expr::YieldFrom(yf) => {
                self.consuming_generator = true;
                self.visit_expr(&yf.value);
                self.consuming_generator = false;
            }
            Expr::ListComp(lc) => self.visit_comprehension_like(&lc.elt, None, &lc.generators),
            Expr::SetComp(sc) => self.visit_comprehension_like(&sc.elt, None, &sc.generators),
            // Bare genexpr is deferred — only walk when consumed.
            Expr::Generator(g) => {
                if self.consuming_generator {
                    self.visit_comprehension_like(&g.elt, None, &g.generators);
                }
            }
            Expr::DictComp(dc) => {
                self.visit_comprehension_like(&dc.value, dc.key.as_deref(), &dc.generators)
            }
            _ => walk_expr(self, expr),
        }
    }
}

impl<'a, 'db> CallCollector<'a, 'db> {
    fn conditions_now(&self) -> Vec<ConditionTag> {
        let mut tags = Vec::new();
        if self.loop_depth > 0 {
            tags.push(ConditionTag::Loop);
        }
        // Prove-only: never emit unconditional/all-paths under any guard.
        if self.cache_miss_depth > 0 {
            tags.push(ConditionTag::CacheMissDependent);
        } else if self.branch_depth > 0 {
            tags.push(ConditionTag::AsyncBranch);
        } else if self.loop_depth == 0 {
            tags.push(ConditionTag::Unconditional);
            tags.push(ConditionTag::AllPaths);
        }
        tags
    }

    /// Walk a statement suite, applying early-exit effects to subsequent stmts.
    fn visit_body_seq(&mut self, body: &'a [Stmt]) {
        let mut after_branch = 0u32;
        let mut after_cache = 0u32;
        let mut after_dead = 0u32;
        for stmt in body {
            if let Stmt::If(if_stmt) = stmt {
                self.visit_if(if_stmt);
                if after_dead == 0
                    && let Some(kind) = self.classify_early_exit(if_stmt)
                {
                    match kind {
                        EarlyExitKind::UnreachableRest => {
                            after_dead += 1;
                            self.dead_depth += 1;
                        }
                        EarlyExitKind::Conditional { cache_miss } => {
                            after_branch += 1;
                            self.branch_depth += 1;
                            if cache_miss {
                                after_cache += 1;
                                self.cache_miss_depth += 1;
                            }
                        }
                    }
                }
            } else {
                self.visit_stmt(stmt);
            }
        }
        self.branch_depth -= after_branch;
        self.cache_miss_depth -= after_cache;
        self.dead_depth -= after_dead;
    }

    fn classify_early_exit(&self, if_stmt: &ast::StmtIf) -> Option<EarlyExitKind> {
        if !body_always_exits(&if_stmt.body) {
            return None;
        }
        // Skip complex elif chains for cheap classification — still mark conditional.
        let has_elif = if_stmt.elif_else_clauses.iter().any(|c| c.test.is_some());
        if self.is_dead_branch_test(&if_stmt.test) {
            // `if False: return` — does not constrain subsequent code.
            return None;
        }
        if self.is_always_truthy(&if_stmt.test) {
            // No live else that continues: rest is unreachable.
            let else_continues = if_stmt
                .elif_else_clauses
                .iter()
                .any(|c| c.test.is_none() && !body_always_exits(&c.body) && !c.body.is_empty());
            if else_continues {
                return Some(EarlyExitKind::Conditional { cache_miss: false });
            }
            return Some(EarlyExitKind::UnreachableRest);
        }
        if has_elif {
            return Some(EarlyExitKind::Conditional { cache_miss: false });
        }
        let cache_miss = is_cache_hit_test(&if_stmt.test);
        Some(EarlyExitKind::Conditional { cache_miss })
    }

    /// Confirm `name` binds to a once-assigned module-level constant via ty.
    /// Returns `None` when unsure (local/param/global/import/other file).
    fn binds_to_module_const(&self, name_expr: &ast::ExprName) -> Option<ModuleConst> {
        let name = name_expr.id.as_str();
        let expected = *self.module_constants.get(name)?;
        if self.current_param_names.iter().any(|p| p == name) {
            return None;
        }
        let resolved = definitions_for_name(
            self.model,
            name,
            name_expr.into(),
            ImportAliasResolution::ResolveAliases,
        );
        if resolved.is_empty() {
            return None;
        }
        for r in &resolved {
            let def = r.definition()?;
            if !def.file_scope(self.model.db()).is_global() {
                return None;
            }
            match def.kind(self.model.db()) {
                DefinitionKind::Assignment(_) | DefinitionKind::AnnotatedAssignment(_) => {}
                _ => return None,
            }
            let path = def.file(self.model.db()).path(self.model.db());
            let FilePath::System(sys) = path else {
                return None;
            };
            let rel = relative_display(self.ctx.db.project_root(), sys);
            if rel != self.ctx.rel {
                // Imported binding — do not prune on name alone.
                return None;
            }
        }
        Some(expected)
    }

    fn is_dead_branch_test(&self, test: &Expr) -> bool {
        match test {
            Expr::BooleanLiteral(b) => !b.value,
            Expr::NoneLiteral(_) => true,
            Expr::Name(name) if name.id.as_str() == "TYPE_CHECKING" => true,
            Expr::Attribute(attr) if attr.attr.as_str() == "TYPE_CHECKING" => true,
            Expr::Name(name) => matches!(
                self.binds_to_module_const(name),
                Some(ModuleConst::False | ModuleConst::None | ModuleConst::Empty)
            ),
            Expr::UnaryOp(u) if matches!(u.op, ast::UnaryOp::Not) => match u.operand.as_ref() {
                Expr::BooleanLiteral(b) => b.value,
                Expr::Name(n) => {
                    matches!(self.binds_to_module_const(n), Some(ModuleConst::True))
                }
                _ => false,
            },
            _ => false,
        }
    }

    fn is_always_truthy(&self, test: &Expr) -> bool {
        match test {
            Expr::BooleanLiteral(b) => b.value,
            Expr::Name(n) => matches!(self.binds_to_module_const(n), Some(ModuleConst::True)),
            Expr::UnaryOp(u) if matches!(u.op, ast::UnaryOp::Not) => {
                self.is_dead_branch_test(u.operand.as_ref())
            }
            _ => false,
        }
    }

    fn visit_comprehension_like(
        &mut self,
        elt: &'a Expr,
        key: Option<&'a Expr>,
        generators: &'a [ast::Comprehension],
    ) {
        let has_filter = generators.iter().any(|g| !g.ifs.is_empty());
        if has_filter {
            self.branch_depth += 1;
        }
        for comp in generators {
            self.consuming_generator = true;
            self.visit_expr(&comp.iter);
            self.consuming_generator = false;
            self.visit_expr(&comp.target);
            for test in &comp.ifs {
                self.visit_expr(test);
            }
        }
        if let Some(k) = key {
            self.visit_expr(k);
        }
        self.visit_expr(elt);
        if has_filter {
            self.branch_depth -= 1;
        }
    }

    fn visit_if(&mut self, if_stmt: &'a ast::StmtIf) {
        let dead = self.is_dead_branch_test(&if_stmt.test);
        let always = self.is_always_truthy(&if_stmt.test);
        let cache_miss = !dead && is_cache_miss_test(&if_stmt.test);
        let param_guard = if dead {
            None
        } else {
            param_guard_from_test(&if_stmt.test, &self.current_param_names)
        };

        if dead {
            self.dead_depth += 1;
        } else {
            self.visit_expr(&if_stmt.test);
        }
        if !dead {
            self.branch_depth += 1;
            if cache_miss {
                self.cache_miss_depth += 1;
            }
        }
        let prev_guard = self.active_param_guard;
        if let Some(g) = param_guard {
            self.active_param_guard = Some(g);
        }
        self.visit_body_seq(&if_stmt.body);
        self.active_param_guard = prev_guard;
        if !dead {
            if cache_miss {
                self.cache_miss_depth -= 1;
            }
            self.branch_depth -= 1;
        }
        if dead {
            self.dead_depth -= 1;
        }

        for clause in &if_stmt.elif_else_clauses {
            let is_else = clause.test.is_none();
            let elif_dead = if is_else {
                // else of a provably-true if is unreachable.
                always
            } else {
                clause
                    .test
                    .as_ref()
                    .is_some_and(|t| self.is_dead_branch_test(t))
            };
            let elif_cache = clause
                .test
                .as_ref()
                .is_some_and(|t| !elif_dead && is_cache_miss_test(t));
            let elif_guard = clause.test.as_ref().and_then(|t| {
                if elif_dead {
                    None
                } else {
                    param_guard_from_test(t, &self.current_param_names)
                }
            });
            if elif_dead {
                self.dead_depth += 1;
            } else if let Some(test) = &clause.test {
                self.visit_expr(test);
            }
            // else / elif always counts as a branch when live — never unconditional.
            if !elif_dead {
                self.branch_depth += 1;
                if elif_cache {
                    self.cache_miss_depth += 1;
                }
            }
            let prev_guard = self.active_param_guard;
            if let Some(g) = elif_guard {
                self.active_param_guard = Some(g);
            }
            self.visit_body_seq(&clause.body);
            self.active_param_guard = prev_guard;
            if !elif_dead {
                if elif_cache {
                    self.cache_miss_depth -= 1;
                }
                self.branch_depth -= 1;
            }
            if elif_dead {
                self.dead_depth -= 1;
            }
        }
    }

    fn visit_try(&mut self, try_stmt: &'a ast::StmtTry) {
        // Any try/except/else/finally path is guarded — not unconditional.
        self.branch_depth += 1;
        self.visit_body_seq(&try_stmt.body);
        self.branch_depth -= 1;
        for handler in &try_stmt.handlers {
            let ast::ExceptHandler::ExceptHandler(h) = handler;
            if let Some(type_) = &h.type_ {
                self.visit_expr(type_);
            }
            self.branch_depth += 1;
            self.visit_body_seq(&h.body);
            self.branch_depth -= 1;
        }
        self.branch_depth += 1;
        self.visit_body_seq(&try_stmt.orelse);
        self.branch_depth -= 1;
        // finally always runs, but keep conservative (still a try-related path).
        self.branch_depth += 1;
        self.visit_body_seq(&try_stmt.finalbody);
        self.branch_depth -= 1;
    }

    fn visit_match(&mut self, match_stmt: &'a ast::StmtMatch) {
        self.visit_expr(&match_stmt.subject);
        for case in &match_stmt.cases {
            self.branch_depth += 1;
            self.visit_body_seq(&case.body);
            self.branch_depth -= 1;
        }
    }

    fn visit_for(&mut self, for_stmt: &'a ast::StmtFor) {
        // Record __iter__/__aiter__ edge when cheap to resolve.
        if let Some(caller) = self.current_fn
            && self.dead_depth == 0
            && !for_stmt.is_async
        {
            let dunder = "__iter__";
            if let Some(callee) = self.resolve_dunder(&for_stmt.iter, dunder) {
                self.push_edge(
                    caller,
                    callee,
                    for_stmt.iter.range(),
                    CallKind::ForIter,
                    false,
                );
            }
        }
        // `async for x in call(...)`: the iter Call is an async use of that one call.
        self.consuming_generator = true;
        if for_stmt.is_async {
            if let Expr::Call(call) = for_stmt.iter.as_ref() {
                self.handle_call(call, true);
                self.walk_call_children(call);
            } else {
                self.visit_expr(&for_stmt.iter);
            }
            self.note_async_for_receivers(for_stmt);
        } else {
            self.visit_expr(&for_stmt.iter);
        }
        self.consuming_generator = false;
        self.loop_depth += 1;
        self.visit_body_seq(&for_stmt.body);
        self.loop_depth -= 1;
        self.visit_body_seq(&for_stmt.orelse);
    }

    fn visit_with(&mut self, with_stmt: &'a ast::StmtWith) {
        if let Some(caller) = self.current_fn
            && self.dead_depth == 0
            && !with_stmt.is_async
        {
            for item in &with_stmt.items {
                if let Some(callee) = self.resolve_dunder(&item.context_expr, "__enter__") {
                    self.push_edge(
                        caller,
                        callee,
                        item.context_expr.range(),
                        CallKind::WithEnter,
                        false,
                    );
                }
                if let Some(callee) = self.resolve_dunder(&item.context_expr, "__exit__") {
                    self.push_edge(
                        caller,
                        callee,
                        item.context_expr.range(),
                        CallKind::WithExit,
                        false,
                    );
                }
            }
        }
        for item in &with_stmt.items {
            // `async with call(...) as x`: mark that one call awaited; bind asyncio receivers.
            if with_stmt.is_async {
                if let Expr::Call(call) = &item.context_expr {
                    self.handle_call(call, true);
                    self.walk_call_children(call);
                } else {
                    self.visit_expr(&item.context_expr);
                }
                self.note_async_with_receiver(item);
            } else {
                self.visit_expr(&item.context_expr);
            }
            if let Some(opt) = &item.optional_vars {
                self.visit_expr(opt);
            }
        }
        self.visit_body_seq(&with_stmt.body);
    }

    fn walk_call_children(&mut self, call: &'a ast::ExprCall) {
        let offload = self.is_offload_callee(&call.func);
        let consumer = is_generator_consumer(&call.func);
        if offload.is_some() {
            for (i, arg) in call.arguments.args.iter().enumerate() {
                if i == 0 && is_callable_ref(arg) {
                    continue;
                }
                if consumer {
                    self.consuming_generator = true;
                }
                self.visit_expr(arg);
                if consumer {
                    self.consuming_generator = false;
                }
            }
            for kw in &call.arguments.keywords {
                self.visit_expr(&kw.value);
            }
        } else if consumer {
            self.in_callee_position = true;
            self.visit_expr(&call.func);
            self.in_callee_position = false;
            self.consuming_generator = true;
            for arg in &call.arguments.args {
                self.visit_expr(arg);
            }
            for kw in &call.arguments.keywords {
                self.visit_expr(&kw.value);
            }
            self.consuming_generator = false;
        } else {
            self.in_callee_position = true;
            self.visit_expr(&call.func);
            self.in_callee_position = false;
            for arg in &call.arguments.args {
                self.visit_expr(arg);
            }
            for kw in &call.arguments.keywords {
                self.visit_expr(&kw.value);
            }
        }
    }

    /// Bind `async with <asyncio redis pipeline()> as name` for receiver-type lookups.
    fn note_async_with_receiver(&mut self, item: &ast::WithItem) {
        let Some(vars) = &item.optional_vars else {
            return;
        };
        let Expr::Name(name) = vars.as_ref() else {
            return;
        };
        if let Some(recv) = self.asyncio_redis_pipeline_receiver(&item.context_expr) {
            self.async_with_receivers
                .insert(name.id.as_str().to_string(), recv);
        }
    }

    fn note_async_for_receivers(&mut self, for_stmt: &ast::StmtFor) {
        // `async for k in client.scan_iter(...)`: bind the iterator receiver when it
        // is an asyncio-redis client so later uses of bound names stay async-keyed.
        // The interesting type is on the method receiver (`client`), not the loop var.
        let _ = for_stmt.target;
        if let Some(recv) = self.asyncio_redis_iter_receiver(&for_stmt.iter) {
            // Stash under a synthetic key used by attribute resolution on the same
            // expression shape is unnecessary — scan_iter itself is the awaited-ish
            // call. Receiver typing for the *call* uses type_instance_qualname /
            // external_qualname on `client.scan_iter` directly. Still record the
            // client name when the iter is `Name.attr(...)` so nested attrs on that
            // same local prefer redis.asyncio.
            if let Expr::Call(call) = for_stmt.iter.as_ref()
                && let Expr::Attribute(attr) = call.func.as_ref()
                && let Expr::Name(n) = attr.value.as_ref()
            {
                self.async_with_receivers
                    .insert(n.id.as_str().to_string(), recv);
            }
        }
    }

    /// Resolve an asyncio-redis client/Pipeline type for `r.pipeline()` contexts.
    fn asyncio_redis_pipeline_receiver(&self, expr: &Expr) -> Option<String> {
        if let Some(ty) = self.type_instance_qualname(expr)
            && is_asyncio_redis_type(&ty)
        {
            return Some(normalize_asyncio_redis_type(&ty));
        }
        if let Expr::Call(call) = expr {
            // Bare Pipeline(...) or client.pipeline()
            if let Expr::Attribute(attr) = call.func.as_ref()
                && attr.attr.as_str() == "pipeline"
            {
                if let Some(recv) = self.type_instance_qualname(&attr.value)
                    && is_asyncio_redis_type(&recv)
                {
                    if recv.contains("Pipeline") {
                        return Some(normalize_asyncio_redis_type(&recv));
                    }
                    return Some("redis.asyncio.client.Pipeline".to_string());
                }
                // Heuristic when ty cannot resolve (dist-packages without venv):
                // `r.pipeline()` where r was imported from redis.asyncio.
                if self.expr_looks_like_asyncio_redis(&attr.value) {
                    return Some("redis.asyncio.client.Pipeline".to_string());
                }
            }
            if let Some(q) = self.external_qualname(&call.func)
                && is_asyncio_redis_type(&q)
                && q.contains("Pipeline")
            {
                return Some(normalize_asyncio_redis_type(&q));
            }
        }
        None
    }

    /// Receiver type for `client.scan_iter(...)` / similar asyncio-redis iterators.
    fn asyncio_redis_iter_receiver(&self, expr: &Expr) -> Option<String> {
        let Expr::Call(call) = expr else {
            return None;
        };
        let Expr::Attribute(attr) = call.func.as_ref() else {
            return None;
        };
        if let Some(recv) = self.type_instance_qualname(&attr.value)
            && is_asyncio_redis_type(&recv)
        {
            return Some(normalize_asyncio_redis_type(&recv));
        }
        if self.expr_looks_like_asyncio_redis(&attr.value) {
            return Some("redis.asyncio.client.Redis".to_string());
        }
        None
    }

    /// True when an expression is known (via ty or import heuristics) to be asyncio redis.
    fn expr_looks_like_asyncio_redis(&self, expr: &Expr) -> bool {
        if let Some(ty) = self.type_instance_qualname(expr)
            && is_asyncio_redis_type(&ty)
        {
            return true;
        }
        // Fall back to definition module path containing redis/asyncio.
        let resolved = match expr {
            Expr::Name(name) => definitions_for_name(
                self.model,
                name.id.as_str(),
                name.into(),
                ImportAliasResolution::ResolveAliases,
            ),
            Expr::Attribute(attr) => definitions_for_attribute(self.model, attr),
            _ => Vec::new(),
        };
        for r in &resolved {
            if let Some(def) = r.definition() {
                let file = def.file(self.model.db());
                if let FilePath::System(sys) = file.path(self.model.db()) {
                    let s = sys.as_str().replace('\\', "/");
                    if s.contains("/redis/asyncio/") || s.contains("/redis/asyncio.py") {
                        return true;
                    }
                }
                if let Some(q) = qualname_from_definition(self.model, def)
                    && is_asyncio_redis_type(&q)
                {
                    return true;
                }
            }
        }
        false
    }

    fn handle_call(&mut self, call: &'a ast::ExprCall, is_awaited: bool) {
        self.pending_is_awaited = is_awaited;
        // Module-level and function-level: task spawns + imperative routes.
        self.note_task_spawn(call);
        self.note_imperative_route(call);

        let Some(caller) = self.current_fn else {
            return;
        };
        if self.dead_depth > 0 || self.in_lambda {
            return;
        }

        // Lambda assigned to a local then called: `f = lambda: ...; f()`.
        if let Expr::Name(name) = call.func.as_ref()
            && let Some(lambda) = self.local_lambdas.get(name.id.as_str()).copied()
        {
            self.analyze_lambda_body(lambda);
        }

        self.pending_literals = call.arguments.args.iter().map(literal_arg).collect();
        self.pending_callables = self.resolve_positional_callables(call.arguments.args.as_ref());

        if let Some(kind) = self.is_offload_callee(&call.func) {
            // Passing a callable param into a real offload → this function is a wrapper.
            if let Some(arg) = call.arguments.args.first()
                && let Expr::Name(n) = arg
                && self.current_param_names.iter().any(|p| p == n.id.as_str())
            {
                self.facts.functions[caller.index()].is_offload_wrapper = true;
            }
            self.push_edge(
                caller,
                CalleeRef::Offload(kind),
                call.range(),
                CallKind::Call,
                true,
            );
            self.pending_literals.clear();
            self.pending_callables.clear();
            return;
        }

        // getattr / setattr dynamic dispatch → unresolved candidate
        if is_getattr_call(&call.func) {
            self.push_edge(
                caller,
                CalleeRef::Unresolved {
                    reason: "unresolved".into(),
                },
                call.range(),
                CallKind::Call,
                false,
            );
            self.pending_literals.clear();
            self.pending_callables.clear();
            return;
        }

        if let Some(callee) = self.resolve_callee(&call.func) {
            let kind = if self.consuming_generator
                && matches!(&callee, CalleeRef::Project(id) if self.facts.function(*id).is_generator)
            {
                CallKind::GeneratorConsume
            } else {
                CallKind::Call
            };
            self.push_edge(caller, callee, call.range(), kind, false);
        }
        self.pending_literals.clear();
        self.pending_callables.clear();
    }

    fn note_local_assign(&mut self, assign: &'a ast::StmtAssign) {
        if assign.targets.len() != 1 {
            return;
        }
        let Expr::Name(n) = &assign.targets[0] else {
            return;
        };
        self.note_local_binding(n.id.as_str(), &assign.value);
    }

    fn note_local_binding(&mut self, name: &str, value: &'a Expr) {
        match value {
            Expr::Lambda(lam) => {
                self.local_lambdas.insert(name.to_string(), lam);
            }
            Expr::Generator(gexpr) => {
                self.local_genexprs.insert(name.to_string(), gexpr);
            }
            _ => {
                self.local_lambdas.remove(name);
                self.local_genexprs.remove(name);
            }
        }
    }

    fn analyze_lambda_body(&mut self, lambda: &'a ast::ExprLambda) {
        let prev_in = self.in_lambda;
        self.in_lambda = false;
        self.visit_expr(&lambda.body);
        self.in_lambda = prev_in;
    }

    fn maybe_property_get(&mut self, attr: &ast::ExprAttribute) {
        let Some(caller) = self.current_fn else {
            return;
        };
        if self.dead_depth > 0 {
            return;
        }
        let resolved = definitions_for_attribute(self.model, attr);
        let mut fid = None;
        for r in &resolved {
            if let Some(def) = r.definition()
                && let Some(id) = self.def_from_definition(def)
            {
                fid = Some(id);
                break;
            }
        }
        let Some(fid) = fid else {
            return;
        };
        let is_prop = self
            .def_map
            .iter()
            .any(|(k, id)| *id == fid && self.property_defs.contains(k));
        if !is_prop {
            return;
        }
        self.pending_literals.clear();
        self.pending_callables.clear();
        self.push_edge(
            caller,
            CalleeRef::Project(fid),
            attr.range(),
            CallKind::PropertyGet,
            false,
        );
    }

    fn note_imperative_route(&mut self, call: &ast::ExprCall) {
        let Some(handler) = route_handler_arg(call) else {
            return;
        };
        let Some(fid) = self.resolve_project_function(handler) else {
            return;
        };
        let f = &mut self.facts.functions[fid.index()];
        if f.label == EntryLabel::Other {
            f.label = EntryLabel::Route;
        }
        if !f.is_async {
            f.is_sync_route = true;
        }
    }

    fn note_task_spawn(&mut self, call: &ast::ExprCall) {
        if !is_task_spawn_callee(&call.func) {
            return;
        }
        for arg in call.arguments.args.iter() {
            // create_task(coro()) — the func of the inner call is the target.
            let target_expr = match arg {
                Expr::Call(inner) => inner.func.as_ref(),
                other => other,
            };
            if let Some(fid) = self.resolve_project_function(target_expr) {
                self.task_targets.0.insert(fid);
            }
        }
    }

    fn resolve_positional_callables(&mut self, args: &'a [Expr]) -> Vec<Option<FunctionId>> {
        let mut out = Vec::with_capacity(args.len());
        for arg in args {
            let fid = match arg {
                Expr::Name(_) | Expr::Attribute(_) => self.resolve_project_function(arg),
                Expr::Call(c) if is_partial_call(c) => c
                    .arguments
                    .args
                    .first()
                    .and_then(|a| self.resolve_project_function(a)),
                Expr::Lambda(lam) => Some(self.ensure_lambda_fact(lam)),
                _ => None,
            };
            out.push(fid);
        }
        out
    }

    fn ensure_lambda_fact(&mut self, lam: &'a ast::ExprLambda) -> FunctionId {
        let key = DefKey {
            file: self.ctx.rel.clone(),
            name_start: u32::from(lam.range.start()),
        };
        if let Some(id) = self.def_map.get(&key).copied() {
            return id;
        }
        let id = FunctionId(self.facts.functions.len() as u32);
        let qualname = format!("<lambda@{}>", self.ctx.line_of(lam.range.start()));
        let param_names: Vec<String> = lam
            .parameters
            .as_ref()
            .map(|p| {
                p.iter_non_variadic_params()
                    .map(|a| a.parameter.name.as_str().to_string())
                    .collect()
            })
            .unwrap_or_default();
        self.def_map.insert(key, id);
        self.qual_map
            .insert((self.ctx.module.clone(), qualname.clone()), id);
        self.facts.functions.push(FunctionFact {
            id,
            file: self.ctx.rel.clone(),
            line: self.ctx.line_of(lam.range.start()),
            module: self.ctx.module.clone(),
            qualname,
            is_async: false,
            is_generator: false,
            label: EntryLabel::Other,
            is_sync_route: false,
            async_twins: Vec::new(),
            param_names,
            is_third_party: false,
            package_name: None,
            package_version: None,
            invokes_params: Vec::new(),
            is_offload_wrapper: false,
        });
        // Analyze lambda body as edges from the synthetic function.
        let prev = self.current_fn;
        let prev_params = std::mem::take(&mut self.current_param_names);
        let prev_in = self.in_lambda;
        self.current_fn = Some(id);
        self.current_param_names = self.facts.function(id).param_names.clone();
        self.in_lambda = false;
        self.visit_expr(&lam.body);
        self.in_lambda = prev_in;
        self.current_fn = prev;
        self.current_param_names = prev_params;
        id
    }

    fn push_edge(
        &mut self,
        caller: FunctionId,
        callee: CalleeRef,
        range: TextRange,
        kind: CallKind,
        is_offload_wrapper: bool,
    ) {
        // Bare generator call (`g = gen()`) must not propagate; only when consumed.
        if let CalleeRef::Project(id) = &callee
            && self.facts.function(*id).is_generator
            && !self.consuming_generator
            && kind != CallKind::GeneratorConsume
        {
            return;
        }

        let key = match &callee {
            CalleeRef::Project(id) => format!("P{}", id.0),
            CalleeRef::External { qualname } => format!("E{qualname}"),
            CalleeRef::Unresolved { reason } => format!("U{reason}"),
            CalleeRef::Offload(k) => format!("O{}", k.as_str()),
            CalleeRef::ParamInvoke { index } => format!("I{index}"),
        };
        let occ = self.occurrence_counter.entry((caller, key)).or_insert(0);
        let occurrence = *occ;
        *occ += 1;

        self.facts.edges.push(CallEdge {
            caller,
            callee,
            file: self.ctx.rel.clone(),
            line: self.ctx.line_of(range.start()),
            text: self.ctx.snippet(range),
            kind,
            conditions: self.conditions_now(),
            occurrence,
            is_offload_wrapper,
            positional_literals: std::mem::take(&mut self.pending_literals),
            positional_callables: std::mem::take(&mut self.pending_callables),
            param_guard: self.active_param_guard,
            is_awaited: self.pending_is_awaited,
        });
        self.pending_is_awaited = false;
    }

    fn is_offload_callee(&self, func: &Expr) -> Option<OffloadKind> {
        // Ty-resolved only — do not treat bare `to_thread` / `run_in_threadpool`
        // attribute names as offload (a project function may share those names).
        let q = self.external_qualname(func)?;
        match q.as_str() {
            "asyncio.to_thread" | "asyncio.threads.to_thread" => Some(OffloadKind::AsyncioToThread),
            "anyio.to_thread.run_sync" => Some(OffloadKind::AnyioToThread),
            "starlette.concurrency.run_in_threadpool" => Some(OffloadKind::StarletteThreadpool),
            "concurrent.futures.Executor.submit"
            | "concurrent.futures.Executor.map"
            | "concurrent.futures.ThreadPoolExecutor.submit"
            | "concurrent.futures.ThreadPoolExecutor.map"
            | "concurrent.futures.ProcessPoolExecutor.submit"
            | "concurrent.futures.ProcessPoolExecutor.map" => Some(OffloadKind::ThreadPoolExecutor),
            other if other.ends_with(".run_in_executor") => Some(OffloadKind::RunInExecutor),
            _ => None,
        }
    }

    fn resolve_dunder(&self, expr: &Expr, dunder: &str) -> Option<CalleeRef> {
        let ty = expr.inferred_type(self.model)?;
        let env = self.model.program_environment();
        // Look up attribute via definitions_for_attribute by synthesizing isn't possible;
        // use member through display of Type::definition on attribute access via try_call-ish.
        // Fall back: resolve `{dunder}` on the expression's type through definitions_for_attribute
        // by building a temporary approach — use Type display + member isn't public enough.
        // Practical approach: if ty is NominalInstance / etc., use definitions_for_attribute
        // only works with ExprAttribute. So check inferred type of a virtual attribute via
        // `definitions_for_attribute` requires AST. Instead, use Type::definition on the
        // expression and then look for methods — too heavy.
        //
        // Cheap path: only resolve when `expr` type has a known definition class and we can
        // find `dunder` in project/stdlib via attribute completions? attribute_completions exists!
        let _ = (ty, env, dunder);
        // For M1: attempt type-based external name `{type}.{dunder}` when we can name the type.
        let type_name = self.type_instance_qualname(expr)?;
        Some(CalleeRef::External {
            qualname: format!("{type_name}.{dunder}"),
        })
    }

    fn type_instance_qualname(&self, expr: &Expr) -> Option<String> {
        let ty = expr.inferred_type(self.model)?;
        let env = self.model.program_environment();
        self.type_to_instance_qualname(ty, &env)
    }

    /// Resolve a concrete instance qualname, including union receivers.
    ///
    /// For `Pipeline | ClusterPipeline` (all asyncio redis) → asyncio Pipeline.
    /// Mixed asyncio/sync redis → `None` (caller must not fall back to a sync
    /// mixin sink; prefer unresolved/candidate).
    fn type_to_instance_qualname(
        &self,
        ty: Type<'_>,
        env: &ty_python_semantic::ProgramEnvironment<'_>,
    ) -> Option<String> {
        if let Some(union) = ty.as_union() {
            let mut names: Vec<String> = Vec::new();
            for elem in union.elements(self.model.db()).iter() {
                // Skip trivial None/Never noise in Optional unions.
                if matches!(elem, Type::Never) {
                    continue;
                }
                if let Some(n) = self.type_to_instance_qualname(*elem, env) {
                    names.push(n);
                } else if elem.definition(self.model.db(), env).is_some() {
                    // Element has a definition we couldn't name — treat as opaque.
                    names.push(String::new());
                }
            }
            names.sort();
            names.dedup();
            names.retain(|n| !n.is_empty());
            if names.is_empty() {
                return None;
            }
            let all_asyncio = names.iter().all(|n| is_asyncio_redis_type(n));
            let any_asyncio = names.iter().any(|n| is_asyncio_redis_type(n));
            let any_sync_redis = names.iter().any(|n| is_sync_redis_type(n));
            if all_asyncio {
                // Prefer Pipeline when any member is a Pipeline.
                if names.iter().any(|n| n.contains("Pipeline")) {
                    return Some("redis.asyncio.client.Pipeline".to_string());
                }
                return Some("redis.asyncio.client.Redis".to_string());
            }
            if any_asyncio && any_sync_redis {
                // Mixed → do not pick a sync sink; signal via None so callers
                // avoid mixin→Redis normalization (emit unresolved instead).
                return None;
            }
            if names.len() == 1 {
                return Some(names.into_iter().next().unwrap());
            }
            // Homogeneous non-redis union: pick the first stable name.
            return names.into_iter().next();
        }

        let def = ty.definition(self.model.db(), env)?;
        match def {
            TypeDefinition::StaticClass(definition) | TypeDefinition::DynamicClass(definition) => {
                let name = definition.name(self.model.db())?;
                let module = module_of_definition(self.model, definition)?;
                Some(format!("{module}.{name}"))
            }
            TypeDefinition::Function(_) => None,
            TypeDefinition::Module(module) => {
                Some(module.name(self.model.db()).as_str().to_string())
            }
            _ => None,
        }
    }

    fn resolve_callee(&mut self, func: &Expr) -> Option<CalleeRef> {
        // Dynamic: getattr(...)  — never guess by name; emit unresolved candidate.
        if matches!(func, Expr::Call(c) if is_getattr_call(&c.func)) {
            return Some(CalleeRef::Unresolved {
                reason: "unresolved".into(),
            });
        }

        if let Some(fid) = self.resolve_project_function(func) {
            return Some(CalleeRef::Project(fid));
        }

        // Higher-order: calling a positional parameter by name.
        if let Expr::Name(name) = func
            && let Some(idx) = self
                .current_param_names
                .iter()
                .position(|p| p == name.id.as_str())
        {
            let index = idx as u32;
            if let Some(fid) = self.current_fn {
                let invokes = &mut self.facts.functions[fid.index()].invokes_params;
                if !invokes.contains(&index) {
                    invokes.push(index);
                }
            }
            return Some(CalleeRef::ParamInvoke { index });
        }

        if let Some(q) = self.external_qualname(func) {
            self.note_follow_target(func, &q);
            return Some(CalleeRef::External { qualname: q });
        }

        // Clearly dynamic / registry-style callables → candidates.
        // Plain unresolved Name/Attribute: omit the edge (cannot prove a sink).
        match func {
            Expr::Subscript(_) | Expr::Call(_) | Expr::BoolOp(_) | Expr::If(_) => {
                Some(CalleeRef::Unresolved {
                    reason: "unresolved".into(),
                })
            }
            _ => None,
        }
    }

    fn note_follow_target(&mut self, func: &Expr, qualname: &str) {
        if self.follow_targets.contains_key(qualname) {
            return;
        }
        let resolved = match func {
            Expr::Name(name) => definitions_for_name(
                self.model,
                name.id.as_str(),
                name.into(),
                ImportAliasResolution::ResolveAliases,
            ),
            Expr::Attribute(attr) => definitions_for_attribute(self.model, attr),
            _ => Vec::new(),
        };
        for r in &resolved {
            let Some(def) = r.definition() else {
                continue;
            };
            let file = def.file(self.model.db());
            let FilePath::System(sys) = file.path(self.model.db()) else {
                continue;
            };
            if !is_followable_site_packages_py(sys.as_str()) {
                continue;
            }
            let kind = def.kind(self.model.db());
            let DefinitionKind::Function(fdef) = kind else {
                continue;
            };
            let parsed = parsed_module(self.model.db(), def.python_file(self.model.db()))
                .load(self.model.db());
            let node = fdef.node(&parsed);
            let std_path = PathBuf::from(sys.as_str());
            let (pkg, ver, fp) = package_meta_for_path(&std_path)
                .map(|(n, v, f)| (Some(n), Some(v), Some(f)))
                .unwrap_or((None, None, None));
            self.follow_targets.insert(
                qualname.to_string(),
                FollowTarget {
                    qualname: qualname.to_string(),
                    path: sys.to_path_buf(),
                    name_start: u32::from(node.name.range.start()),
                    package_name: pkg,
                    package_version: ver,
                    package_fingerprint: fp,
                },
            );
            return;
        }
    }

    fn resolve_project_function(&self, func: &Expr) -> Option<FunctionId> {
        let resolved = match func {
            Expr::Name(name) => definitions_for_name(
                self.model,
                name.id.as_str(),
                name.into(),
                ImportAliasResolution::ResolveAliases,
            ),
            Expr::Attribute(attr) => definitions_for_attribute(self.model, attr),
            _ => {
                // Fall through to type-based definition.
                Vec::new()
            }
        };

        for r in &resolved {
            if let Some(def) = r.definition()
                && let Some(fid) = self.def_from_definition(def)
            {
                return Some(fid);
            }
        }

        // Type-based: FunctionLiteral / BoundMethod → definition.
        let ty = func.inferred_type(self.model)?;
        let env = self.model.program_environment();
        let typedef = ty.definition(self.model.db(), &env)?;
        if let TypeDefinition::Function(def) = typedef {
            return self.def_from_definition(def);
        }
        None
    }

    fn def_from_definition(
        &self,
        def: ty_python_core::definition::Definition<'db>,
    ) -> Option<FunctionId> {
        let file = def.file(self.model.db());
        let path = file.path(self.model.db());
        let FilePath::System(sys) = path else {
            // Vendored / stub — not a project function.
            return None;
        };
        let rel = relative_display(self.ctx.db.project_root(), sys);
        let parsed =
            parsed_module(self.model.db(), def.python_file(self.model.db())).load(self.model.db());
        let kind = def.kind(self.model.db());
        let DefinitionKind::Function(fdef) = kind else {
            return None;
        };
        let node = fdef.node(&parsed);
        let key = DefKey {
            file: rel,
            name_start: u32::from(node.name.range.start()),
        };
        self.def_map.get(&key).copied()
    }

    fn external_qualname(&self, func: &Expr) -> Option<String> {
        // For attributes, ALWAYS prefer `{receiver_type}.{attr}` over the
        // defining class. Mixin-inherited methods (e.g. redis CoreCommands.set
        // on a Pipeline instance) must key off the *receiver* so Pipeline
        // buffering is not mistaken for Redis IO.
        if let Expr::Attribute(attr) = func {
            // Prefer bindings from `async with r.pipeline() as pipe`.
            if let Expr::Name(n) = attr.value.as_ref()
                && let Some(recv) = self.async_with_receivers.get(n.id.as_str())
            {
                return Some(format!("{recv}.{}", attr.attr.as_str()));
            }
            // Union receivers (Pipeline | ClusterPipeline, Redis | RedisCluster).
            if let Some(ty) = attr.value.inferred_type(self.model) {
                let env = self.model.program_environment();
                if ty.as_union().is_some() {
                    match self.type_to_instance_qualname(ty, &env) {
                        Some(recv) => {
                            let recv = if is_asyncio_redis_type(&recv) {
                                normalize_asyncio_redis_type(&recv)
                            } else {
                                recv
                            };
                            return Some(format!("{recv}.{}", attr.attr.as_str()));
                        }
                        None => {
                            // Mixed asyncio|sync (or unnameable union): never fall
                            // through to CoreCommands → sync Redis.
                            if self.expr_looks_like_asyncio_redis(&attr.value) {
                                return Some(format!(
                                    "redis.asyncio.client.Pipeline.{}",
                                    attr.attr.as_str()
                                ));
                            }
                            return None;
                        }
                    }
                }
            }
            if let Some(recv) = self.type_instance_qualname(&attr.value) {
                let recv = if is_asyncio_redis_type(&recv) {
                    normalize_asyncio_redis_type(&recv)
                } else {
                    recv
                };
                return Some(format!("{recv}.{}", attr.attr.as_str()));
            }
            // dist-packages / weak typing: still recognize asyncio redis receivers.
            if self.expr_looks_like_asyncio_redis(&attr.value) {
                return Some(format!("redis.asyncio.client.Redis.{}", attr.attr.as_str()));
            }
        }

        // Prefer definitions_for_* → file_to_module + name + enclosing class.
        let resolved = match func {
            Expr::Name(name) => definitions_for_name(
                self.model,
                name.id.as_str(),
                name.into(),
                ImportAliasResolution::ResolveAliases,
            ),
            Expr::Attribute(attr) => definitions_for_attribute(self.model, attr),
            _ => Vec::new(),
        };

        for r in &resolved {
            if let Some(def) = r.definition()
                && let Some(q) = qualname_from_definition(self.model, def)
            {
                // Bare redis mixin defs are not sync-IO without a receiver type.
                if is_redis_commands_mixin(&q) {
                    continue;
                }
                return Some(q);
            }
        }

        let ty = func.inferred_type(self.model)?;
        let env = self.model.program_environment();
        match ty.definition(self.model.db(), &env)? {
            TypeDefinition::Function(def) => {
                let q = qualname_from_definition(self.model, def)?;
                if is_redis_commands_mixin(&q) {
                    return None;
                }
                Some(q)
            }
            TypeDefinition::Module(module) => {
                Some(module.name(self.model.db()).as_str().to_string())
            }
            TypeDefinition::StaticClass(def) | TypeDefinition::DynamicClass(def) => {
                let name = def.name(self.model.db())?;
                let module = module_of_definition(self.model, def)?;
                Some(format!("{module}.{name}"))
            }
            _ => None,
        }
    }
}

fn is_sync_redis_type(ty: &str) -> bool {
    (ty.contains("redis.") || ty.contains(".redis"))
        && !is_asyncio_redis_type(ty)
        && (ty.contains("Redis") || ty.contains("Pipeline") || ty.contains("StrictRedis"))
}

fn is_redis_commands_mixin(q: &str) -> bool {
    q.contains("redis.commands.core.CoreCommands")
        || q.contains("redis.commands.core.AsyncCoreCommands")
        || q.contains("redis.commands.cluster.")
}

fn qualname_from_definition<'db>(
    model: &SemanticModel<'db>,
    def: ty_python_core::definition::Definition<'db>,
) -> Option<String> {
    let db = model.db();
    let parsed = parsed_module(db, def.python_file(db)).load(db);
    let kind = def.kind(db);
    let DefinitionKind::Function(fdef) = kind else {
        // Could be an import binding pointing at a function — use def.name.
        let name = def.name(db)?;
        let module = module_of_definition(model, def)?;
        return Some(format!("{module}.{name}"));
    };
    let node = fdef.node(&parsed);
    let func_name = node.name.as_str();
    let module = module_of_definition(model, def)?;

    // Enclosing class via parent scopes.
    let index = semantic_index(db, def.program_file(db));
    let mut scope_id = def.file_scope(db);
    let mut class_parts: Vec<String> = Vec::new();
    while let Some(parent) = index.parent_scope_id(scope_id) {
        let scope = index.scope(parent);
        if let Some(class_ref) = scope.node().as_class() {
            let class_node = class_ref.node(&parsed);
            class_parts.push(class_node.name.as_str().to_string());
        }
        scope_id = parent;
    }
    class_parts.reverse();
    if class_parts.is_empty() {
        Some(format!("{module}.{func_name}"))
    } else {
        Some(format!("{module}.{}.{}", class_parts.join("."), func_name))
    }
}

fn module_of_definition<'db>(
    model: &SemanticModel<'db>,
    def: ty_python_core::definition::Definition<'db>,
) -> Option<String> {
    let db = model.db();
    let file = def.file(db);
    match file.path(db) {
        FilePath::System(sys) => {
            let s = sys.as_str().replace('\\', "/");
            if let Some(rest) = third_party_module_path(&s) {
                return Some(module_name_from_relative(rest));
            }
            // Best-effort: file stem when we cannot recover the package path.
            Some(module_name_from_relative(
                sys.file_name().unwrap_or(sys.as_str()),
            ))
        }
        FilePath::Vendored(vendored) => {
            let s = vendored.as_str().replace('\\', "/");
            // typeshed: stdlib/time.pyi, stdlib/pathlib/__init__.pyi
            let rest = s
                .strip_prefix("stdlib/")
                .or_else(|| {
                    // stubs/requests/requests/... → skip first component
                    s.strip_prefix("stubs/")
                        .and_then(|r| r.split_once('/').map(|(_, rest)| rest))
                })
                .unwrap_or(&s);
            let rest = rest
                .strip_suffix(".pyi")
                .or_else(|| rest.strip_suffix(".py"))
                .unwrap_or(rest);
            Some(module_name_from_relative(rest))
        }
        FilePath::SystemVirtual(_) => None,
    }
}

fn is_callable_ref(expr: &Expr) -> bool {
    match expr {
        Expr::Name(_) | Expr::Attribute(_) | Expr::Lambda(_) => true,
        Expr::Call(call) => is_partial_call(call),
        _ => false,
    }
}

fn is_partial_call(call: &ast::ExprCall) -> bool {
    match call.func.as_ref() {
        Expr::Name(n) if n.id.as_str() == "partial" => true,
        Expr::Attribute(a) if a.attr.as_str() == "partial" => true,
        _ => false,
    }
}

/// Extract the route handler expression from imperative FastAPI/Starlette registration.
fn route_handler_arg(call: &ast::ExprCall) -> Option<&Expr> {
    let attr_name = match call.func.as_ref() {
        Expr::Attribute(a) => Some(a.attr.as_str()),
        Expr::Name(n) => Some(n.id.as_str()),
        _ => None,
    };
    let name = attr_name?;

    match name {
        "add_api_route" | "add_route" => {
            // add_api_route(path, endpoint, ...) — handler at index 1, or endpoint=
            if let Some(arg) = call.arguments.args.get(1) {
                return Some(arg);
            }
            call.arguments
                .keywords
                .iter()
                .find(|k| k.arg.as_ref().is_some_and(|a| a.as_str() == "endpoint"))
                .map(|k| &k.value)
        }
        "Route" | "WebSocketRoute" => {
            // Route(path, endpoint) or Route(path, endpoint=...)
            if let Some(arg) = call.arguments.args.get(1) {
                return Some(arg);
            }
            if let Some(arg) = call.arguments.args.first() {
                // Rare: Route(endpoint) — only if single positional and not a string.
                if !matches!(arg, Expr::StringLiteral(_)) {
                    return Some(arg);
                }
            }
            call.arguments
                .keywords
                .iter()
                .find(|k| {
                    k.arg
                        .as_ref()
                        .is_some_and(|a| matches!(a.as_str(), "endpoint" | "app"))
                })
                .map(|k| &k.value)
        }
        _ => None,
    }
}

/// Build an adjacency index: caller → edge indices. O(E) once, O(degree) lookup after.
fn edges_by_caller(facts: &ProjectFacts) -> Vec<Vec<usize>> {
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
fn extend_edges_by_caller(facts: &ProjectFacts, idx: &mut Vec<Vec<usize>>, from_edge: usize) {
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

/// After first-party extraction, follow External callees into site-packages `.py` sources.
///
/// Determinism: BTreeSet/BTreeMap everywhere; sorted queues; sink-table Externals are
/// **never** rewritten to Project (so rule-table findings cannot be dropped by follow).
/// Budget applies only to non-sink follow candidates, sorted by (caller_count desc, qualname).
fn follow_third_party(
    db: &AsyncDoctorDb,
    facts: &mut ProjectFacts,
    def_map: &mut HashMap<DefKey, FunctionId>,
    qual_map: &mut HashMap<(String, String), FunctionId>,
    follow_targets: &mut HashMap<String, FollowTarget>,
    cache_dir: Option<&Path>,
) {
    let sinks = BuiltinSinkTable::new();
    let mut qual_to_id: BTreeMap<String, FunctionId> = BTreeMap::new();
    let mut id_to_external_key: HashMap<FunctionId, String> = HashMap::new();
    let cache_root = effective_cache_root(cache_dir);
    let mut cache_batch = PackageCacheBatch::new(cache_root.clone());
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
                    // Do not follow awaited known-async APIs (redis.asyncio /
                    // AsyncConnection / …) — their internals are not sync-IO evidence.
                    // Unknown third-party async (e.g. libz.fetch → time.sleep) still follows.
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
            &cache_root,
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
            if let Some(id) =
                extract_third_party_function(db, facts, def_map, qual_map, follow_targets, &target)
            {
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
        if let Some(id) =
            extract_third_party_function(db, facts, def_map, qual_map, follow_targets, &target)
        {
            qual_to_id.insert(q.clone(), id);
            id_to_external_key.insert(id, q.clone());
            reachable_fns.insert(id);
            extend_edges_by_caller(facts, &mut by_caller, edge_before);
        }
    }

    // Rewrite after the initial package/cache wave.
    rewrite_external_to_project(facts, &qual_to_id, &sinks);

    // Further waves until fixpoint or budget — a single extra wave misses deep
    // site-packages chains (liba→libb→libc→libd→sleep) and made cold miss
    // paths that warm transitive cache materialization could still expose.
    const FOLLOW_BUDGET: usize = 256;
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
        let remaining = FOLLOW_BUDGET.saturating_sub(followed);
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
                &cache_root,
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
            if followed >= FOLLOW_BUDGET {
                break;
            }
            let Some(target) = follow_targets.get(&qualname).cloned() else {
                continue;
            };
            let edge_before = facts.edges.len();
            if let Some(id) =
                extract_third_party_function(db, facts, def_map, qual_map, follow_targets, &target)
            {
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
            if qual_to_id.contains_key(q) || followed >= FOLLOW_BUDGET {
                continue;
            }
            let Some(target) = follow_targets.get(q).cloned() else {
                continue;
            };
            let edge_before = facts.edges.len();
            if let Some(id) =
                extract_third_party_function(db, facts, def_map, qual_map, follow_targets, &target)
            {
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
fn rewrite_external_to_project(
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
fn extract_third_party_function(
    db: &AsyncDoctorDb,
    facts: &mut ProjectFacts,
    def_map: &mut HashMap<DefKey, FunctionId>,
    qual_map: &mut HashMap<(String, String), FunctionId>,
    follow_targets: &mut HashMap<String, FollowTarget>,
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
        async_with_receivers: HashMap::new(),
        occurrence_counter: HashMap::new(),
        pending_literals: Vec::new(),
        pending_callables: Vec::new(),
        local_lambdas: HashMap::new(),
        local_genexprs: HashMap::new(),
    };
    visitor.visit_body_seq(&func.body);
    Some(id)
}

fn is_getattr_call(func: &Expr) -> bool {
    match func {
        Expr::Name(n) => n.id.as_str() == "getattr",
        Expr::Attribute(a) => a.attr.as_str() == "getattr",
        _ => false,
    }
}

/// True for redis.asyncio types (client, pipeline, connection, …).
fn is_asyncio_redis_type(ty: &str) -> bool {
    ty.contains("redis.asyncio") || ty.contains("asyncio.client.Pipeline")
}

/// Canonicalize asyncio-redis type strings so site- and dist-packages agree.
fn normalize_asyncio_redis_type(ty: &str) -> String {
    if ty.contains("Pipeline") {
        return "redis.asyncio.client.Pipeline".to_string();
    }
    if ty.contains("Redis") || ty.ends_with(".StrictRedis") {
        return "redis.asyncio.client.Redis".to_string();
    }
    if let Some(rest) = ty.strip_prefix("redis.asyncio.") {
        return format!("redis.asyncio.{rest}");
    }
    ty.to_string()
}

fn is_task_spawn_callee(func: &Expr) -> bool {
    match func {
        Expr::Attribute(attr) => matches!(
            attr.attr.as_str(),
            "create_task" | "ensure_future" | "gather" | "start_soon" | "start" | "add_task"
        ),
        Expr::Name(name) => matches!(name.id.as_str(), "create_task" | "ensure_future" | "gather"),
        _ => false,
    }
}
