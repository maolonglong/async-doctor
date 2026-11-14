//! Coroutine-return proofs, class protocol checks, and `dangling-task` detection.

use async_doctor_core::{CallKind, CalleeRef, FunctionId};
use async_doctor_rules::RULE_DANGLING_TASK;
use ruff_db::files::FilePath;
use ruff_db::parsed::parsed_module;
use ruff_db::source::source_text;
use ruff_python_ast::{self as ast, Expr, Stmt};
use ruff_text_size::Ranged;
use ty_python_core::definition::{Definition, DefinitionKind};
use ty_python_semantic::types::{Type, TypeDefinition};
use ty_python_semantic::{
    HasType, ImportAliasResolution, definitions_for_attribute, definitions_for_name,
};

use super::super::CallCollector;
use super::super::loop_guards::{expr_is_get_event_loop, expr_is_get_running_loop};
use super::{decorator_preserves_coroutine, return_annotation_mentions_async_iter};
use crate::extract::constants::body_has_yield;

impl<'a, 'db> CallCollector<'a, 'db> {
    pub(super) fn expr_is_proven_coroutine(&self, expr: &Expr) -> bool {
        if let Expr::Call(call) = expr
            && self.call_returns_proven_coroutine(call)
        {
            return true;
        }
        expr.inferred_type(self.model)
            .is_some_and(|ty| self.type_is_proven_coroutine(ty))
    }

    fn type_is_proven_coroutine(&self, ty: Type<'db>) -> bool {
        if let Some(union) = ty.as_union() {
            let elements = union.elements(self.model.db());
            let mut live = elements
                .iter()
                .copied()
                .filter(|ty| !matches!(ty, Type::Never))
                .peekable();
            return live.peek().is_some() && live.all(|ty| self.type_is_proven_coroutine(ty));
        }
        if matches!(
            ty,
            Type::ClassLiteral(_) | Type::SubclassOf(_) | Type::GenericAlias(_)
        ) {
            return false;
        }
        let env = self.model.program_environment();
        self.type_to_instance_qualname(ty, &env).is_some_and(|q| {
            matches!(
                q.as_str(),
                "builtins.coroutine"
                    | "types.CoroutineType"
                    | "typing.Coroutine"
                    | "collections.abc.Coroutine"
                    | "_collections_abc.Coroutine"
            )
        })
    }

    /// True when `call` is proven to return a coroutine object (not an async
    /// generator, not `@asynccontextmanager`, not `@async_to_sync`).
    pub(crate) fn call_returns_proven_coroutine(&self, call: &ast::ExprCall) -> bool {
        if !self.resolve_project_constructors(&call.func).is_empty() {
            return false;
        }
        if let Expr::Attribute(attr) = call.func.as_ref()
            && let Some(recv_ty) = attr.value.inferred_type(self.model)
            && let Some(union) = recv_ty.as_union()
        {
            let env = self.model.program_environment();
            for elem in union.elements(self.model.db()).iter() {
                if matches!(elem, Type::Never) {
                    continue;
                }
                if elem.is_none(self.model.db()) || elem.definition(self.model.db(), &env).is_none()
                {
                    return false;
                }
            }
        }
        let Some(defs) = self.resolve_callee_function_defs(&call.func) else {
            return false;
        };
        if defs.is_empty() {
            return false;
        }
        for def in defs {
            let DefinitionKind::Function(fdef) = def.kind(self.model.db()) else {
                return false;
            };
            let py_file = def.python_file(self.model.db());
            let parsed = parsed_module(self.model.db(), py_file).load(self.model.db());
            let node = fdef.node(&parsed);
            if !node.is_async || body_has_yield(&node.body) {
                return false;
            }
            if !node
                .decorator_list
                .iter()
                .all(|d| decorator_preserves_coroutine(&d.expression))
            {
                return false;
            }
            let src = source_text(self.model.db(), def.file(self.model.db()));
            if return_annotation_mentions_async_iter(src.as_str(), node) {
                return false;
            }
        }
        if let Some(call_ty) = Expr::Call(call.clone()).inferred_type(self.model) {
            let env = self.model.program_environment();
            if let Some(q) = self.type_to_instance_qualname(call_ty, &env)
                && matches!(
                    q.as_str(),
                    "builtins.async_generator"
                        | "types.AsyncGeneratorType"
                        | "typing.AsyncIterator"
                        | "typing.AsyncIterable"
                        | "typing.AsyncGenerator"
                        | "collections.abc.AsyncIterator"
                        | "collections.abc.AsyncIterable"
                        | "collections.abc.AsyncGenerator"
                        | "_collections_abc.AsyncIterator"
                        | "_collections_abc.AsyncIterable"
                        | "_collections_abc.AsyncGenerator"
                )
            {
                return false;
            }
        }
        true
    }

    pub(in crate::extract::collector) fn resolve_callee_function_defs(
        &self,
        func: &Expr,
    ) -> Option<Vec<Definition<'db>>> {
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
        let mut defs = Vec::new();
        for r in &resolved {
            let def = r.definition()?;
            if !matches!(def.kind(self.model.db()), DefinitionKind::Function(_)) {
                return None;
            }
            defs.push(def);
        }
        if !defs.is_empty() {
            return Some(defs);
        }
        let ty = func.inferred_type(self.model)?;
        let env = self.model.program_environment();
        if let Some(union) = ty.as_union() {
            for elem in union.elements(self.model.db()).iter() {
                if matches!(elem, Type::Never) {
                    continue;
                }
                let TypeDefinition::Function(def) = elem.definition(self.model.db(), &env)? else {
                    return None;
                };
                defs.push(def);
            }
            return (!defs.is_empty()).then_some(defs);
        }
        let TypeDefinition::Function(def) = ty.definition(self.model.db(), &env)? else {
            return None;
        };
        Some(vec![def])
    }

    pub(in crate::extract::collector) fn expr_resolves_to_project_class_matching(
        &self,
        expr: &Expr,
        pred: impl Fn(&ast::StmtClassDef) -> bool + Copy,
    ) -> bool {
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
        if resolved.is_empty() {
            return false;
        }
        for r in &resolved {
            let Some(def) = r.definition() else {
                return false;
            };
            if !self.class_def_matches(def, pred) {
                return false;
            }
        }
        true
    }

    pub(in crate::extract::collector) fn class_def_matches(
        &self,
        def: Definition<'db>,
        pred: impl Fn(&ast::StmtClassDef) -> bool,
    ) -> bool {
        let file = def.file(self.model.db());
        let FilePath::System(sys) = file.path(self.model.db()) else {
            return false;
        };
        if sys.extension() == Some("pyi") {
            return false;
        }
        let DefinitionKind::Class(cdef) = def.kind(self.model.db()) else {
            return false;
        };
        let parsed =
            parsed_module(self.model.db(), def.python_file(self.model.db())).load(self.model.db());
        let class_node = cdef.node(&parsed);
        pred(class_node)
    }

    pub(super) fn class_node_is_async_only_cm(class_node: &ast::StmtClassDef) -> bool {
        if !class_node.bases().is_empty() {
            return false;
        }
        let mut has_aenter = false;
        let mut has_enter = false;
        for stmt in &class_node.body {
            if let Stmt::FunctionDef(f) = stmt {
                match f.name.as_str() {
                    "__aenter__" => has_aenter = true,
                    "__enter__" => has_enter = true,
                    _ => {}
                }
            }
        }
        has_aenter && !has_enter
    }

    pub(super) fn class_node_is_async_only_iterable(class_node: &ast::StmtClassDef) -> bool {
        if !class_node.bases().is_empty() {
            return false;
        }
        let mut has_aiter = false;
        let mut has_sync_iter = false;
        for stmt in &class_node.body {
            if let Stmt::FunctionDef(f) = stmt {
                match f.name.as_str() {
                    "__aiter__" => has_aiter = true,
                    "__iter__" | "__getitem__" => has_sync_iter = true,
                    _ => {}
                }
            }
        }
        has_aiter && !has_sync_iter
    }

    pub(super) fn async_protocol_target_fids(&self, expr: &Expr) -> Vec<FunctionId> {
        let mut out = Vec::new();
        if let Expr::Call(call) = expr {
            if let Some(defs) = self.resolve_callee_function_defs(&call.func) {
                for def in defs {
                    if let Some(fid) = self.def_from_definition(def)
                        && !out.contains(&fid)
                    {
                        out.push(fid);
                    }
                }
            }
            let resolved = match call.func.as_ref() {
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
                    self.collect_class_protocol_fids(def, &mut out);
                }
            }
        }
        if let Some(ty) = expr.inferred_type(self.model) {
            let env = self.model.program_environment();
            self.collect_type_protocol_fids(ty, &env, &mut out);
        }
        out
    }

    fn collect_type_protocol_fids(
        &self,
        ty: Type<'db>,
        env: &ty_python_semantic::ProgramEnvironment<'db>,
        out: &mut Vec<FunctionId>,
    ) {
        if let Some(union) = ty.as_union() {
            for elem in union.elements(self.model.db()).iter() {
                if !matches!(elem, Type::Never) {
                    self.collect_type_protocol_fids(*elem, env, out);
                }
            }
            return;
        }
        match ty.definition(self.model.db(), env) {
            Some(TypeDefinition::Function(def)) => {
                if let Some(fid) = self.def_from_definition(def)
                    && !out.contains(&fid)
                {
                    out.push(fid);
                }
            }
            Some(TypeDefinition::StaticClass(def) | TypeDefinition::DynamicClass(def)) => {
                self.collect_class_protocol_fids(def, out);
            }
            _ => {}
        }
    }

    fn collect_class_protocol_fids(&self, def: Definition<'db>, out: &mut Vec<FunctionId>) {
        let db = self.model.db();
        let FilePath::System(sys) = def.file(db).path(db) else {
            return;
        };
        if sys.extension() == Some("pyi") {
            return;
        }
        let DefinitionKind::Class(cdef) = def.kind(db) else {
            return;
        };
        let rel = crate::extract::path::relative_display(self.ctx.db.project_root(), sys);
        let parsed = parsed_module(db, def.python_file(db)).load(db);
        let class_node = cdef.node(&parsed);
        for stmt in &class_node.body {
            if let Stmt::FunctionDef(f) = stmt
                && matches!(
                    f.name.as_str(),
                    "__aenter__" | "__aexit__" | "__aiter__" | "__anext__" | "__init__"
                )
            {
                let key = crate::extract::ctx::DefKey {
                    file: rel.clone(),
                    name_start: u32::from(f.name.range.start()),
                };
                if let Some(fid) = self.def_get(&key)
                    && !out.contains(&fid)
                {
                    out.push(fid);
                }
            }
        }
    }

    pub(crate) fn maybe_record_dangling_task(&mut self, expr_stmt: &ast::StmtExpr) {
        let Some(caller) = self.current_fn else {
            return;
        };
        if self.dead_depth > 0 || self.in_lambda || self.off_loop_thread_depth > 0 {
            return;
        }
        let Expr::Call(call) = expr_stmt.value.as_ref() else {
            return;
        };
        if !self.is_dangling_task_call(call) {
            return;
        }
        self.clear_pending_call_args();
        self.push_edge(
            caller,
            CalleeRef::External {
                qualname: RULE_DANGLING_TASK.to_string(),
            },
            call.range(),
            CallKind::Call,
            false,
            Some(false),
        );
    }

    fn is_dangling_task_call(&mut self, call: &ast::ExprCall) -> bool {
        if let Some(q) = self.external_qualname(&call.func) {
            if matches!(
                q.as_str(),
                "asyncio.create_task"
                    | "asyncio.tasks.create_task"
                    | "asyncio.ensure_future"
                    | "asyncio.tasks.ensure_future"
                    | "asyncio.events.AbstractEventLoop.create_task"
                    | "asyncio.base_events.BaseEventLoop.create_task"
            ) {
                return true;
            }
            if q.ends_with(".create_task")
                && !q.contains("TaskGroup")
                && !q.contains("task_group")
                && q.starts_with("asyncio.")
            {
                return true;
            }
        }

        if let Expr::Attribute(attr) = call.func.as_ref()
            && attr.attr.as_str() == "create_task"
        {
            match attr.value.as_ref() {
                Expr::Name(n) => {
                    let id = n.id.as_str();
                    if self.running_loop_locals.contains(id) || self.event_loop_locals.contains(id)
                    {
                        return true;
                    }
                }
                other => {
                    if expr_is_get_running_loop(other) || expr_is_get_event_loop(other) {
                        return true;
                    }
                }
            }
        }

        false
    }
}
