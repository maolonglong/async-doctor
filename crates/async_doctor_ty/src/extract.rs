//! Extract first-party call-graph facts using ty's semantic model.
//!
//! All ty/ruff usage stays in this crate. Core sees only plain [`ProjectFacts`].

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::Context;
use async_doctor_core::{
    CallEdge, CallKind, CalleeRef, ConditionTag, EntryLabel, FunctionFact, FunctionId, LiteralArg,
    OffloadKind, ParamGuard, ProjectFacts,
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
use ty_python_semantic::types::TypeDefinition;
use ty_python_semantic::{
    HasType, ImportAliasResolution, SemanticModel, definitions_for_attribute, definitions_for_name,
};

use crate::db::AsyncDoctorDb;

/// Scan a project directory or a single `.py` file into graded facts.
pub fn extract_path(path: &Path) -> anyhow::Result<ProjectFacts> {
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
        return extract_files(&db, vec![path.as_std_path().to_path_buf()]);
    }

    let db = AsyncDoctorDb::open(&path, None)?;
    let files = discover_python_files(db.project_root().as_std_path())?;
    extract_files(&db, files)
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

fn extract_files(db: &AsyncDoctorDb, files: Vec<PathBuf>) -> anyhow::Result<ProjectFacts> {
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
            def_map: &def_map,
            qual_map: &qual_map,
            task_targets: &mut task_targets,
            module_constants: &module_constants,
            current_fn: None,
            current_param_names: Vec::new(),
            class_stack: Vec::new(),
            dead_depth: 0,
            loop_depth: 0,
            branch_depth: 0,
            cache_miss_depth: 0,
            active_param_guard: None,
            consuming_generator: false,
            in_lambda: false,
            skip_lambda_body: false,
            occurrence_counter: HashMap::new(),
            pending_literals: Vec::new(),
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

fn collect_functions(
    ctx: &FileCtx<'_>,
    suite: &[Stmt],
    facts: &mut ProjectFacts,
    def_map: &mut HashMap<DefKey, FunctionId>,
    qual_map: &mut HashMap<(String, String), FunctionId>,
    class_methods: &mut HashMap<(String, String), Vec<String>>,
) {
    struct Collector<'a, 'db> {
        ctx: &'a FileCtx<'db>,
        facts: &'a mut ProjectFacts,
        def_map: &'a mut HashMap<DefKey, FunctionId>,
        qual_map: &'a mut HashMap<(String, String), FunctionId>,
        class_methods: &'a mut HashMap<(String, String), Vec<String>>,
        class_stack: Vec<String>,
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
                    let qualname = if self.class_stack.is_empty() {
                        func.name.as_str().to_string()
                    } else {
                        format!("{}.{}", self.class_stack.join("."), func.name.as_str())
                    };
                    if !self.class_stack.is_empty() {
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
                    });

                    // Nested functions.
                    self.visit_body(&func.body);
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
        class_stack: Vec::new(),
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

fn collect_module_constants(suite: &[Stmt]) -> HashMap<String, ModuleConst> {
    let mut assign_count: HashMap<String, u32> = HashMap::new();
    let mut values: HashMap<String, ModuleConst> = HashMap::new();

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

    values.retain(|k, _| assign_count.get(k).copied().unwrap_or(0) == 1);
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

fn is_always_truthy(test: &Expr, constants: &HashMap<String, ModuleConst>) -> bool {
    match test {
        Expr::BooleanLiteral(b) => b.value,
        Expr::Name(n) => matches!(constants.get(n.id.as_str()), Some(ModuleConst::True)),
        Expr::UnaryOp(u) if matches!(u.op, ast::UnaryOp::Not) => {
            is_dead_branch_test(u.operand.as_ref(), constants)
        }
        _ => false,
    }
}

struct CallCollector<'a, 'db> {
    ctx: &'a FileCtx<'db>,
    model: &'a SemanticModel<'db>,
    facts: &'a mut ProjectFacts,
    def_map: &'a HashMap<DefKey, FunctionId>,
    qual_map: &'a HashMap<(String, String), FunctionId>,
    task_targets: &'a mut TaskTargets,
    module_constants: &'a HashMap<String, ModuleConst>,
    current_fn: Option<FunctionId>,
    current_param_names: Vec<String>,
    class_stack: Vec<String>,
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
    occurrence_counter: HashMap<(FunctionId, String), u32>,
    /// Literals for the call currently being recorded (set by handle_call).
    pending_literals: Vec<Option<LiteralArg>>,
}

impl<'a, 'db> Visitor<'a> for CallCollector<'a, 'db> {
    fn visit_stmt(&mut self, stmt: &'a Stmt) {
        if self.skip_lambda_body {
            return;
        }
        match stmt {
            Stmt::FunctionDef(func) => {
                let qualname = if self.class_stack.is_empty() {
                    func.name.as_str().to_string()
                } else {
                    format!("{}.{}", self.class_stack.join("."), func.name.as_str())
                };
                let fid = self
                    .qual_map
                    .get(&(self.ctx.module.clone(), qualname))
                    .copied();
                let prev = self.current_fn;
                let prev_params = std::mem::take(&mut self.current_param_names);
                self.current_fn = fid;
                self.current_param_names = fid
                    .map(|id| self.facts.function(id).param_names.clone())
                    .unwrap_or_default();
                for s in &func.body {
                    self.visit_stmt(s);
                }
                self.current_fn = prev;
                self.current_param_names = prev_params;
            }
            Stmt::ClassDef(class) => {
                self.class_stack.push(class.name.as_str().to_string());
                for s in &class.body {
                    self.visit_stmt(s);
                }
                self.class_stack.pop();
            }
            Stmt::If(if_stmt) => self.visit_if(if_stmt),
            Stmt::For(for_stmt) => {
                self.visit_for(for_stmt);
            }
            Stmt::While(while_stmt) => {
                self.loop_depth += 1;
                self.visit_expr(&while_stmt.test);
                for s in &while_stmt.body {
                    self.visit_stmt(s);
                }
                self.loop_depth -= 1;
                for s in &while_stmt.orelse {
                    self.visit_stmt(s);
                }
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
            Expr::Call(call) => {
                self.handle_call(call);
                // Still walk arguments for nested calls (except offloaded callable args).
                let offload = self.is_offload_callee(&call.func);
                let consumer = is_generator_consumer(&call.func);
                if offload.is_some() {
                    // Walk non-callable-ref args fully; skip Name/Attribute/Lambda/Partial first arg.
                    for (i, arg) in call.arguments.args.iter().enumerate() {
                        if i == 0 && is_callable_ref(arg) {
                            // Offloaded: if lambda, do not analyze its body.
                            continue;
                        }
                        // Eager: to_thread(f()) — walk the Call.
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
                    // list(gen()) / next(gen()) — nested generator calls are consumed.
                    self.visit_expr(&call.func);
                    self.consuming_generator = true;
                    for arg in &call.arguments.args {
                        self.visit_expr(arg);
                    }
                    for kw in &call.arguments.keywords {
                        self.visit_expr(&kw.value);
                    }
                    self.consuming_generator = false;
                } else {
                    walk_expr(self, expr);
                }
            }
            Expr::YieldFrom(yf) => {
                self.consuming_generator = true;
                self.visit_expr(&yf.value);
                self.consuming_generator = false;
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

    fn visit_if(&mut self, if_stmt: &'a ast::StmtIf) {
        let dead = is_dead_branch_test(&if_stmt.test, self.module_constants);
        let always = is_always_truthy(&if_stmt.test, self.module_constants);
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
        for s in &if_stmt.body {
            self.visit_stmt(s);
        }
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
                    .is_some_and(|t| is_dead_branch_test(t, self.module_constants))
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
            for s in &clause.body {
                self.visit_stmt(s);
            }
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
        for s in &try_stmt.body {
            self.visit_stmt(s);
        }
        self.branch_depth -= 1;
        for handler in &try_stmt.handlers {
            let ast::ExceptHandler::ExceptHandler(h) = handler;
            if let Some(type_) = &h.type_ {
                self.visit_expr(type_);
            }
            self.branch_depth += 1;
            for s in &h.body {
                self.visit_stmt(s);
            }
            self.branch_depth -= 1;
        }
        self.branch_depth += 1;
        for s in &try_stmt.orelse {
            self.visit_stmt(s);
        }
        self.branch_depth -= 1;
        // finally always runs, but keep conservative (still a try-related path).
        self.branch_depth += 1;
        for s in &try_stmt.finalbody {
            self.visit_stmt(s);
        }
        self.branch_depth -= 1;
    }

    fn visit_match(&mut self, match_stmt: &'a ast::StmtMatch) {
        self.visit_expr(&match_stmt.subject);
        for case in &match_stmt.cases {
            self.branch_depth += 1;
            for s in &case.body {
                self.visit_stmt(s);
            }
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
        // Iterating consumes a generator produced by a call in the iter expr.
        self.consuming_generator = true;
        self.visit_expr(&for_stmt.iter);
        self.consuming_generator = false;
        self.loop_depth += 1;
        for s in &for_stmt.body {
            self.visit_stmt(s);
        }
        self.loop_depth -= 1;
        for s in &for_stmt.orelse {
            self.visit_stmt(s);
        }
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
            self.visit_expr(&item.context_expr);
            if let Some(opt) = &item.optional_vars {
                self.visit_expr(opt);
            }
        }
        for s in &with_stmt.body {
            self.visit_stmt(s);
        }
    }

    fn handle_call(&mut self, call: &'a ast::ExprCall) {
        let Some(caller) = self.current_fn else {
            // Module-level: still detect task spawning of async defs.
            self.note_task_spawn(call);
            return;
        };
        if self.dead_depth > 0 || self.in_lambda {
            return;
        }

        self.note_task_spawn(call);

        self.pending_literals = call.arguments.args.iter().map(literal_arg).collect();

        if let Some(kind) = self.is_offload_callee(&call.func) {
            self.push_edge(
                caller,
                CalleeRef::Offload(kind),
                call.range(),
                CallKind::Call,
                true,
            );
            self.pending_literals.clear();
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
        {
            return;
        }

        let key = match &callee {
            CalleeRef::Project(id) => format!("P{}", id.0),
            CalleeRef::External { qualname } => format!("E{qualname}"),
            CalleeRef::Unresolved { reason } => format!("U{reason}"),
            CalleeRef::Offload(k) => format!("O{}", k.as_str()),
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
            param_guard: self.active_param_guard,
        });
    }

    fn is_offload_callee(&self, func: &Expr) -> Option<OffloadKind> {
        // Prefer type-resolved external name when available.
        if let Some(q) = self.external_qualname(func) {
            return match q.as_str() {
                "asyncio.to_thread" | "asyncio.threads.to_thread" => {
                    Some(OffloadKind::AsyncioToThread)
                }
                "anyio.to_thread.run_sync" => Some(OffloadKind::AnyioToThread),
                "starlette.concurrency.run_in_threadpool" => Some(OffloadKind::StarletteThreadpool),
                "concurrent.futures.Executor.submit"
                | "concurrent.futures.Executor.map"
                | "concurrent.futures.ThreadPoolExecutor.submit"
                | "concurrent.futures.ThreadPoolExecutor.map" => {
                    Some(OffloadKind::ThreadPoolExecutor)
                }
                other if other.ends_with(".run_in_executor") => Some(OffloadKind::RunInExecutor),
                other if other.ends_with(".submit") || other.ends_with(".map") => {
                    // Heuristic only when type says Executor — already handled above.
                    let _ = other;
                    None
                }
                _ => None,
            };
        }
        // AST fallback for common patterns when types don't resolve.
        match func {
            Expr::Attribute(attr) => {
                let name = attr.attr.as_str();
                match name {
                    "to_thread" => Some(OffloadKind::AsyncioToThread),
                    "run_in_executor" => Some(OffloadKind::RunInExecutor),
                    "run_sync"
                        if matches!(
                            attr.value.as_ref(),
                            Expr::Attribute(a) if a.attr.as_str() == "to_thread"
                        ) =>
                    {
                        Some(OffloadKind::AnyioToThread)
                    }
                    "run_in_threadpool" => Some(OffloadKind::StarletteThreadpool),
                    // Do not guess bare `.submit`/`.map` without a typed Executor.
                    _ => None,
                }
            }
            Expr::Name(name) if name.id.as_str() == "run_in_threadpool" => {
                Some(OffloadKind::StarletteThreadpool)
            }
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
        let def = ty.definition(self.model.db(), &env)?;
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

    fn resolve_callee(&self, func: &Expr) -> Option<CalleeRef> {
        // Dynamic: getattr(...)  — never guess by name; emit unresolved candidate.
        if matches!(func, Expr::Call(c) if is_getattr_call(&c.func)) {
            return Some(CalleeRef::Unresolved {
                reason: "unresolved".into(),
            });
        }

        if let Some(fid) = self.resolve_project_function(func) {
            return Some(CalleeRef::Project(fid));
        }

        if let Some(q) = self.external_qualname(func) {
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
        // For attributes, prefer `{receiver_type}.{attr}` so methods like
        // `Path.read_text` keep the class even when the stub definition is odd.
        if let Expr::Attribute(attr) = func
            && let Some(recv) = self.type_instance_qualname(&attr.value)
        {
            let candidate = format!("{recv}.{}", attr.attr.as_str());
            // Also try definition-based (may include mixins / aliases).
            let resolved = definitions_for_attribute(self.model, attr);
            for r in &resolved {
                if let Some(def) = r.definition()
                    && let Some(q) = qualname_from_definition(self.model, def)
                {
                    // Prefer the longer / more specific name when both exist.
                    if q.ends_with(&format!(".{}", attr.attr.as_str()))
                        && q.matches('.').count() >= candidate.matches('.').count()
                    {
                        return Some(q);
                    }
                }
            }
            return Some(candidate);
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
                return Some(q);
            }
        }

        let ty = func.inferred_type(self.model)?;
        let env = self.model.program_environment();
        match ty.definition(self.model.db(), &env)? {
            TypeDefinition::Function(def) => qualname_from_definition(self.model, def),
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
            if let Some(idx) = s.rfind("/site-packages/") {
                let rest = &s[idx + "/site-packages/".len()..];
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

fn is_dead_branch_test(test: &Expr, constants: &HashMap<String, ModuleConst>) -> bool {
    match test {
        Expr::BooleanLiteral(b) => !b.value,
        Expr::NoneLiteral(_) => true,
        Expr::Name(name) if name.id.as_str() == "TYPE_CHECKING" => true,
        Expr::Attribute(attr) if attr.attr.as_str() == "TYPE_CHECKING" => true,
        Expr::Name(name) => matches!(
            constants.get(name.id.as_str()),
            Some(ModuleConst::False | ModuleConst::None | ModuleConst::Empty)
        ),
        Expr::UnaryOp(u) if matches!(u.op, ast::UnaryOp::Not) => {
            // `if not X` is dead only when X is provably truthy.
            match u.operand.as_ref() {
                Expr::BooleanLiteral(b) => b.value,
                Expr::Name(n) => matches!(constants.get(n.id.as_str()), Some(ModuleConst::True)),
                _ => false,
            }
        }
        _ => false,
    }
}

fn is_callable_ref(expr: &Expr) -> bool {
    match expr {
        Expr::Name(_) | Expr::Attribute(_) | Expr::Lambda(_) => true,
        Expr::Call(call) => {
            // functools.partial(...)
            match call.func.as_ref() {
                Expr::Name(n) if n.id.as_str() == "partial" => true,
                Expr::Attribute(a) if a.attr.as_str() == "partial" => true,
                _ => false,
            }
        }
        _ => false,
    }
}

fn is_getattr_call(func: &Expr) -> bool {
    match func {
        Expr::Name(n) => n.id.as_str() == "getattr",
        Expr::Attribute(a) => a.attr.as_str() == "getattr",
        _ => false,
    }
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
