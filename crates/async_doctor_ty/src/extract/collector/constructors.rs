//! Class constructor (`__new__`, `__init__`, `__post_init__`), `super()`, and
//! inherited-method resolution for project classes.
//!
//! `ty` resolves a class name `Worker(...)` to `DefinitionKind::Class`, not a
//! function, and `definitions_for_attribute` skips `Type::BoundSuper` (`super()`)
//! and `cls` (`SubclassOf(TypeVar(Self))`) inside `@classmethod`s. This module
//! walks the project class hierarchy (and `bases()` across files) against the
//! Pass-1 `def_map` / `qual_map` tables.

use std::collections::HashSet;

use async_doctor_core::FunctionId;
use ruff_db::files::FilePath;
use ruff_db::parsed::parsed_module;
use ruff_python_ast::{self as ast, Expr, Stmt};
use ty_python_core::definition::{Definition, DefinitionKind};
use ty_python_semantic::types::{Type, TypeDefinition};
use ty_python_semantic::{
    HasType, ImportAliasResolution, SemanticModel, definitions_for_attribute, definitions_for_name,
};

use super::CallCollector;
use crate::extract::ctx::DefKey;
use crate::extract::path::relative_display;

/// Lifecycle methods executed when a class is instantiated (`Cls(...)`).
const CONSTRUCTOR_METHODS: &[&str] = &["__new__", "__init__", "__post_init__"];

#[derive(Clone, Copy)]
enum ClassTarget<'db> {
    Def(Definition<'db>),
    Enclosing,
}

impl<'a, 'db> CallCollector<'a, 'db> {
    /// Resolve all project lifecycle methods (`__new__`, `__init__`, `__post_init__`)
    /// invoked when `func` is called as a class constructor (`Cls(...)` or `cls(...)`).
    pub(crate) fn resolve_project_constructors(&self, func: &Expr) -> Vec<FunctionId> {
        let Some(target) = self.resolve_class_target(func) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for &method in CONSTRUCTOR_METHODS {
            let mut visited = HashSet::new();
            let fids = match target {
                ClassTarget::Def(def) => {
                    self.lookup_method_in_class_def(def, method, true, &mut visited)
                }
                ClassTarget::Enclosing => {
                    self.lookup_method_in_enclosing_class(method, true, &mut visited)
                }
            };
            for fid in fids {
                if !out.contains(&fid) {
                    out.push(fid);
                }
            }
        }
        out
    }

    /// Resolve `super().<attr>` or `cls.<attr>` / inherited project methods when
    /// `definitions_for_attribute` returns empty.
    pub(crate) fn resolve_inherited_or_super_method(
        &self,
        attr: &ast::ExprAttribute,
    ) -> Option<FunctionId> {
        let method = attr.attr.as_str();
        let recv = attr.value.as_ref();

        // `super().method(...)` or `super(Cls, self).method(...)`
        if self.expr_is_super_call(recv) {
            let mut visited = HashSet::new();
            return self
                .lookup_method_in_enclosing_class(method, false, &mut visited)
                .into_iter()
                .next();
        }

        // `cls.method(...)` or `self.method(...)` inside an enclosing class when
        // `ty` leaves `cls`/`self` as a `Self` TypeVar.
        if let Expr::Name(n) = recv
            && matches!(n.id.as_str(), "cls" | "self")
            && self.current_param_names.first().map(String::as_str) == Some(n.id.as_str())
            && !self.class_stack.is_empty()
        {
            let mut visited = HashSet::new();
            if let Some(fid) = self
                .lookup_method_in_enclosing_class(method, true, &mut visited)
                .into_iter()
                .next()
            {
                return Some(fid);
            }
        }

        // `Sub.create(...)` or `instance.inherited(...)` where `recv` resolves
        // to a project class (either as a class object or an instance).
        if let Some(def) = self.resolve_receiver_class_def(recv) {
            let mut visited = HashSet::new();
            if let Some(fid) = self
                .lookup_method_in_class_def(def, method, true, &mut visited)
                .into_iter()
                .next()
            {
                return Some(fid);
            }
        }

        // Module-qualified function decorated with `@functools.cache` / `@lru_cache`:
        // `definitions_for_attribute` returns the `Function` definition, handled
        // in `resolve_project_function`; if `recv` is a module and `definitions_for_attribute`
        // missed it, check `qual_map` for `(module, method)`.
        if let Some(ty) = recv.inferred_type(self.model) {
            let env = self.model.program_environment();
            if let Some(TypeDefinition::Module(module)) = ty.definition(self.model.db(), &env) {
                let mod_name = module.name(self.model.db()).as_str().to_string();
                if let Some(fid) = self.qual_get(&(mod_name, method.to_string())) {
                    return Some(fid);
                }
            }
        }

        None
    }

    /// Syntactic fallback for `importlib_resources` when the backport package is
    /// not installed in the active environment (typeshed only bundles stdlib
    /// `importlib.resources`).
    pub(crate) fn resolve_uninstalled_resource_qualname(&self, func: &Expr) -> Option<String> {
        match func {
            Expr::Name(n) => {
                let name = n.id.as_str();
                if !matches!(
                    name,
                    "as_file"
                        | "open_text"
                        | "open_binary"
                        | "read_text"
                        | "read_binary"
                        | "contents"
                        | "is_resource"
                ) {
                    return None;
                }
                if self.name_imported_from_importlib_resources(n, name) {
                    return Some(format!("importlib.resources.{name}"));
                }
                None
            }
            Expr::Attribute(attr) => {
                let method = attr.attr.as_str();
                if matches!(
                    method,
                    "as_file"
                        | "open_text"
                        | "open_binary"
                        | "read_text"
                        | "read_binary"
                        | "contents"
                        | "is_resource"
                ) && self.expr_is_importlib_resources_module(&attr.value)
                {
                    return Some(format!("importlib.resources.{method}"));
                }
                if matches!(
                    method,
                    "read_text" | "read_bytes" | "open" | "iterdir" | "is_file" | "is_dir"
                ) && self.expr_is_traversable_chain(&attr.value)
                {
                    return Some(format!("importlib.resources.abc.Traversable.{method}"));
                }
                None
            }
            _ => None,
        }
    }

    fn expr_is_traversable_chain(&self, expr: &Expr) -> bool {
        match expr {
            Expr::Call(call) => match call.func.as_ref() {
                Expr::Name(n) if n.id.as_str() == "files" => {
                    self.name_imported_from_importlib_resources(n, "files")
                }
                Expr::Attribute(attr) if attr.attr.as_str() == "files" => {
                    self.expr_is_importlib_resources_module(&attr.value)
                }
                Expr::Attribute(attr) if attr.attr.as_str() == "joinpath" => {
                    self.expr_is_traversable_chain(&attr.value)
                }
                _ => false,
            },
            Expr::BinOp(bin) if matches!(bin.op, ast::Operator::Div) => {
                self.expr_is_traversable_chain(&bin.left)
            }
            _ => false,
        }
    }

    fn expr_is_importlib_resources_module(&self, expr: &Expr) -> bool {
        let Expr::Name(n) = expr else {
            return false;
        };
        let resolved = definitions_for_name(
            self.model,
            n.id.as_str(),
            n.into(),
            ImportAliasResolution::PreserveAliases,
        );
        for r in &resolved {
            let Some(def) = r.definition() else {
                continue;
            };
            let db = self.model.db();
            let parsed = parsed_module(db, def.python_file(db)).load(db);
            match def.kind(db) {
                DefinitionKind::Import(imp) => {
                    let alias = imp.alias(&parsed);
                    if alias.name.as_str() == "importlib_resources"
                        || alias.name.as_str() == "importlib.resources"
                    {
                        return true;
                    }
                }
                DefinitionKind::ImportFrom(imp) => {
                    let node = imp.import(&parsed);
                    let alias = imp.alias(&parsed);
                    if node.module.as_ref().map(|m| m.as_str()) == Some("importlib")
                        && alias.name.as_str() == "resources"
                    {
                        return true;
                    }
                }
                _ => {}
            }
        }
        false
    }

    fn name_imported_from_importlib_resources(&self, n: &ast::ExprName, symbol: &str) -> bool {
        let resolved = definitions_for_name(
            self.model,
            n.id.as_str(),
            n.into(),
            ImportAliasResolution::PreserveAliases,
        );
        for r in &resolved {
            let Some(def) = r.definition() else {
                continue;
            };
            let db = self.model.db();
            if let DefinitionKind::ImportFrom(imp) = def.kind(db) {
                let parsed = parsed_module(db, def.python_file(db)).load(db);
                let node = imp.import(&parsed);
                let alias = imp.alias(&parsed);
                let module = node.module.as_ref().map(|m| m.as_str()).unwrap_or("");
                if (module == "importlib_resources" || module == "importlib.resources")
                    && alias.name.as_str() == symbol
                {
                    return true;
                }
            }
        }
        false
    }

    fn resolve_class_target(&self, expr: &Expr) -> Option<ClassTarget<'db>> {
        let expr = match expr {
            // Generic instantiation `Worker[int](...)`
            Expr::Subscript(sub) => sub.value.as_ref(),
            other => other,
        };

        // `cls(...)` inside a method/classmethod where `cls` is the first parameter.
        if let Expr::Name(name) = expr
            && name.id.as_str() == "cls"
            && self.current_param_names.first().map(String::as_str) == Some("cls")
            && !self.class_stack.is_empty()
        {
            return Some(ClassTarget::Enclosing);
        }

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
            if let Some(def) = r.definition()
                && matches!(def.kind(self.model.db()), DefinitionKind::Class(_))
            {
                return Some(ClassTarget::Def(def));
            }
        }

        // Type-based fallback: only when `expr` is a class object itself
        // (`ClassLiteral`, `SubclassOf`, `GenericAlias`), never a `NominalInstance`
        // (`w = Worker(); w()`).
        let ty = expr.inferred_type(self.model)?;
        if !matches!(
            ty,
            Type::ClassLiteral(_) | Type::SubclassOf(_) | Type::GenericAlias(_)
        ) {
            return None;
        }
        let env = self.model.program_environment();
        match ty.definition(self.model.db(), &env)? {
            TypeDefinition::StaticClass(def) | TypeDefinition::DynamicClass(def) => {
                Some(ClassTarget::Def(def))
            }
            _ => None,
        }
    }

    fn resolve_receiver_class_def(&self, recv: &Expr) -> Option<Definition<'db>> {
        if let Some(ClassTarget::Def(def)) = self.resolve_class_target(recv) {
            return Some(def);
        }
        let ty = recv.inferred_type(self.model)?;
        let env = self.model.program_environment();
        match ty.definition(self.model.db(), &env)? {
            TypeDefinition::StaticClass(def) | TypeDefinition::DynamicClass(def) => Some(def),
            _ => None,
        }
    }

    fn expr_is_super_call(&self, expr: &Expr) -> bool {
        match expr {
            Expr::Call(call) => match call.func.as_ref() {
                Expr::Name(n) => n.id.as_str() == "super",
                Expr::Attribute(attr) => {
                    attr.attr.as_str() == "super"
                        && matches!(attr.value.as_ref(), Expr::Name(n) if n.id.as_str() == "builtins")
                }
                _ => false,
            },
            _ => false,
        }
    }

    fn lookup_method_in_enclosing_class(
        &self,
        method: &str,
        include_self: bool,
        visited: &mut HashSet<(String, u32)>,
    ) -> Vec<FunctionId> {
        if self.class_stack.is_empty() {
            return Vec::new();
        }
        let parsed = parsed_module(
            self.model.db(),
            self.ctx.program_file.python_file(self.model.db()),
        )
        .load(self.model.db());
        let Some(class_def) = find_class_in_suite(parsed.suite(), self.class_stack.as_slice())
        else {
            return Vec::new();
        };
        self.lookup_method_in_stmt_class(
            self.model,
            &self.ctx.rel,
            self.class_stack.as_slice(),
            class_def,
            method,
            include_self,
            visited,
        )
    }

    fn lookup_method_in_class_def(
        &self,
        def: Definition<'db>,
        method: &str,
        include_self: bool,
        visited: &mut HashSet<(String, u32)>,
    ) -> Vec<FunctionId> {
        let db = self.model.db();
        let DefinitionKind::Class(cdef) = def.kind(db) else {
            return Vec::new();
        };
        let FilePath::System(sys) = def.file(db).path(db) else {
            return Vec::new();
        };
        let rel = relative_display(self.ctx.db.project_root(), sys);
        let parsed = parsed_module(db, def.python_file(db)).load(db);
        let class_node = cdef.node(&parsed);
        let visit_key = (rel.clone(), u32::from(class_node.name.range.start()));
        if !visited.insert(visit_key) {
            return Vec::new();
        }

        if include_self
            && let Some(fid) = self.find_method_in_class_body(&rel, &class_node.body, method)
        {
            return vec![fid];
        }

        let class_model = SemanticModel::new(db, def.program_file(db));
        let mut out = Vec::new();
        for base in class_node.bases() {
            if let Some(base_def) = self.resolve_base_class_def(&class_model, base) {
                let fids = self.lookup_method_in_class_def(base_def, method, true, visited);
                if !fids.is_empty() {
                    out.extend(fids);
                    break;
                }
            }
        }
        out
    }

    #[allow(clippy::too_many_arguments)]
    fn lookup_method_in_stmt_class(
        &self,
        model: &SemanticModel<'db>,
        rel: &str,
        class_path: &[String],
        class_node: &ast::StmtClassDef,
        method: &str,
        include_self: bool,
        visited: &mut HashSet<(String, u32)>,
    ) -> Vec<FunctionId> {
        let visit_key = (rel.to_string(), u32::from(class_node.name.range.start()));
        if !visited.insert(visit_key) {
            return Vec::new();
        }
        if include_self {
            if let Some(fid) = self.find_method_in_class_body(rel, &class_node.body, method) {
                return vec![fid];
            }
            if !class_path.is_empty() {
                let qual = format!("{}.{}", class_path.join("."), method);
                if let Some(fid) = self.qual_get(&(self.ctx.module.clone(), qual)) {
                    return vec![fid];
                }
            }
        }
        let mut out = Vec::new();
        for base in class_node.bases() {
            if let Some(base_def) = self.resolve_base_class_def(model, base) {
                let fids = self.lookup_method_in_class_def(base_def, method, true, visited);
                if !fids.is_empty() {
                    out.extend(fids);
                    break;
                }
            }
        }
        out
    }

    fn resolve_base_class_def(
        &self,
        model: &SemanticModel<'db>,
        base: &Expr,
    ) -> Option<Definition<'db>> {
        let base = match base {
            Expr::Subscript(sub) => sub.value.as_ref(),
            other => other,
        };
        let resolved = match base {
            Expr::Name(name) => definitions_for_name(
                model,
                name.id.as_str(),
                name.into(),
                ImportAliasResolution::ResolveAliases,
            ),
            Expr::Attribute(attr) => definitions_for_attribute(model, attr),
            _ => Vec::new(),
        };
        for r in &resolved {
            if let Some(def) = r.definition()
                && matches!(def.kind(model.db()), DefinitionKind::Class(_))
            {
                return Some(def);
            }
        }
        let ty = base.inferred_type(model)?;
        let env = model.program_environment();
        match ty.definition(model.db(), &env)? {
            TypeDefinition::StaticClass(def) | TypeDefinition::DynamicClass(def) => Some(def),
            _ => None,
        }
    }

    fn find_method_in_class_body(
        &self,
        rel: &str,
        body: &[Stmt],
        method: &str,
    ) -> Option<FunctionId> {
        let mut found = None;
        for stmt in body {
            match stmt {
                Stmt::FunctionDef(func) if func.name.as_str() == method => {
                    let key = DefKey {
                        file: rel.to_string(),
                        name_start: u32::from(func.name.range.start()),
                    };
                    if let Some(fid) = self.def_get(&key) {
                        found = Some(fid);
                    }
                }
                Stmt::If(if_stmt) => {
                    if let Some(fid) = self.find_method_in_class_body(rel, &if_stmt.body, method) {
                        found = Some(fid);
                    }
                    for clause in &if_stmt.elif_else_clauses {
                        if let Some(fid) = self.find_method_in_class_body(rel, &clause.body, method)
                        {
                            found = Some(fid);
                        }
                    }
                }
                _ => {}
            }
        }
        found
    }
}

fn find_class_in_suite<'a>(
    body: &'a [Stmt],
    class_stack: &[String],
) -> Option<&'a ast::StmtClassDef> {
    let (head, rest) = class_stack.split_first()?;
    for stmt in body {
        match stmt {
            Stmt::ClassDef(class) if class.name.as_str() == head.as_str() => {
                if rest.is_empty() {
                    return Some(class);
                }
                if let Some(inner) = find_class_in_suite(&class.body, rest) {
                    return Some(inner);
                }
            }
            Stmt::FunctionDef(func) => {
                if let Some(found) = find_class_in_suite(&func.body, class_stack) {
                    return Some(found);
                }
            }
            Stmt::If(if_stmt) => {
                if let Some(found) = find_class_in_suite(&if_stmt.body, class_stack) {
                    return Some(found);
                }
                for clause in &if_stmt.elif_else_clauses {
                    if let Some(found) = find_class_in_suite(&clause.body, class_stack) {
                        return Some(found);
                    }
                }
            }
            Stmt::With(w) => {
                if let Some(found) = find_class_in_suite(&w.body, class_stack) {
                    return Some(found);
                }
            }
            Stmt::Try(t) => {
                if let Some(found) = find_class_in_suite(&t.body, class_stack) {
                    return Some(found);
                }
                for h in &t.handlers {
                    let ast::ExceptHandler::ExceptHandler(eh) = h;
                    if let Some(found) = find_class_in_suite(&eh.body, class_stack) {
                        return Some(found);
                    }
                }
                if let Some(found) = find_class_in_suite(&t.orelse, class_stack) {
                    return Some(found);
                }
                if let Some(found) = find_class_in_suite(&t.finalbody, class_stack) {
                    return Some(found);
                }
            }
            _ => {}
        }
    }
    None
}
