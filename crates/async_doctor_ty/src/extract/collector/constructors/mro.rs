//! Class hierarchy MRO method lookup and `@dataclass` / `@attrs` `__post_init__`
//! detection across project files.

use std::collections::HashSet;

use async_doctor_core::FunctionId;
use ruff_db::files::FilePath;
use ruff_db::parsed::parsed_module;
use ruff_python_ast::{self as ast, Expr, Stmt};
use ty_python_core::definition::{Definition, DefinitionKind};
use ty_python_semantic::types::TypeDefinition;
use ty_python_semantic::{
    HasType, ImportAliasResolution, SemanticModel, definitions_for_attribute, definitions_for_name,
};

use super::ClassTarget;
use crate::extract::collector::CallCollector;
use crate::extract::ctx::DefKey;
use crate::extract::path::relative_display;

impl<'a, 'db> CallCollector<'a, 'db> {
    pub(super) fn target_has_post_init_semantics(&self, target: ClassTarget<'db>) -> bool {
        let mut visited = HashSet::new();
        match target {
            ClassTarget::Def(def) => self.class_def_has_post_init(def, &mut visited),
            ClassTarget::Enclosing => {
                let Some(class_def) = self.class_def_stack.last().copied() else {
                    return false;
                };
                self.stmt_class_has_post_init(self.model, &self.ctx.rel, class_def, &mut visited)
            }
        }
    }

    fn class_def_has_post_init(
        &self,
        def: Definition<'db>,
        visited: &mut HashSet<(String, u32)>,
    ) -> bool {
        let db = self.model.db();
        let DefinitionKind::Class(cdef) = def.kind(db) else {
            return false;
        };
        let FilePath::System(sys) = def.file(db).path(db) else {
            return false;
        };
        let rel = relative_display(self.ctx.db.project_root(), sys);
        let parsed = parsed_module(db, def.python_file(db)).load(db);
        let class_node = cdef.node(&parsed);
        let class_model = SemanticModel::new(db, def.program_file(db));
        self.stmt_class_has_post_init(&class_model, &rel, class_node, visited)
    }

    fn stmt_class_has_post_init(
        &self,
        model: &SemanticModel<'db>,
        rel: &str,
        class_node: &ast::StmtClassDef,
        visited: &mut HashSet<(String, u32)>,
    ) -> bool {
        let visit_key = (rel.to_string(), u32::from(class_node.name.range.start()));
        if !visited.insert(visit_key) {
            return false;
        }
        if class_node
            .decorator_list
            .iter()
            .any(|d| decorator_enables_post_init(&d.expression))
        {
            return true;
        }
        for base in class_node.bases() {
            if let Some(base_def) = self.resolve_base_class_def(model, base)
                && self.class_def_has_post_init(base_def, visited)
            {
                return true;
            }
        }
        false
    }

    pub(super) fn lookup_method_in_enclosing_class(
        &self,
        method: &str,
        include_self: bool,
        visited: &mut HashSet<(String, u32)>,
    ) -> Vec<FunctionId> {
        let Some(class_def) = self.class_def_stack.last().copied() else {
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

    pub(super) fn lookup_method_in_class_def(
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

fn decorator_enables_post_init(expr: &Expr) -> bool {
    match expr {
        Expr::Call(call) => decorator_enables_post_init(&call.func),
        Expr::Name(n) => matches!(
            n.id.as_str(),
            "dataclass" | "define" | "frozen" | "mutable" | "attrs"
        ),
        Expr::Attribute(attr) => matches!(
            attr.attr.as_str(),
            "dataclass" | "define" | "frozen" | "mutable" | "s" | "attrs"
        ),
        _ => false,
    }
}
