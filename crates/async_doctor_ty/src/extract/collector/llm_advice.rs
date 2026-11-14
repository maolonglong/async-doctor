//! Pass-2 AST seed detection for high-precision LLM async correctness advice:
//! - `offload-stateful-session`
//! - `swallowed-cancelled-error`
//! - `useless-local-async-lock` (see `local_lock` submodule)

mod local_lock;

use std::collections::HashSet;

use async_doctor_core::{CallKind, CalleeRef, FunctionId, OffloadKind};
use ruff_python_ast::{self as ast, Expr, Stmt};
use ruff_text_size::{Ranged, TextRange};
use ty_python_semantic::HasType;
use ty_python_semantic::types::{Type, TypeDefinition};

use super::CallCollector;
use crate::extract::helpers::module_of_definition;

fn canonical_sync_db_handle(qualname: &str) -> Option<&'static str> {
    match qualname {
        "sqlalchemy.orm.Session" | "sqlalchemy.orm.session.Session" => {
            Some("sqlalchemy.orm.Session")
        }
        "sqlmodel.Session" | "sqlmodel.orm.session.Session" => Some("sqlmodel.Session"),
        "sqlite3.Connection" | "_sqlite3.Connection" => Some("sqlite3.Connection"),
        "sqlite3.Cursor" | "_sqlite3.Cursor" => Some("sqlite3.Cursor"),
        _ => None,
    }
}

fn canonical_sync_db_handle_from_method(qualname: &str) -> Option<&'static str> {
    let (owner, _method) = qualname.rsplit_once('.')?;
    canonical_sync_db_handle(owner)
}

impl<'a, 'db> CallCollector<'a, 'db> {
    // ── 1. offload-stateful-session ─────────────────────────────────────────

    pub(crate) fn maybe_record_offload_stateful_session(
        &mut self,
        caller: FunctionId,
        effective_target: Option<&'a Expr>,
        kind: OffloadKind,
        offload_range: TextRange,
    ) {
        if self.function(caller).is_installed_third_party() {
            return;
        }
        let Some(target) = effective_target else {
            return;
        };
        let (callable_expr, lambda_params) = if let Some(lam) = self.resolve_lambda_expr(target) {
            let Expr::Call(inner_call) = lam.body.as_ref() else {
                return;
            };
            let params: HashSet<&str> = lam
                .parameters
                .as_ref()
                .map(|p| {
                    p.iter_non_variadic_params()
                        .map(|a| a.parameter.name.as_str())
                        .collect()
                })
                .unwrap_or_default();
            (inner_call.func.as_ref(), Some(params))
        } else {
            (target, None)
        };

        let Expr::Attribute(attr) = callable_expr else {
            return;
        };
        if let Some(params) = &lambda_params {
            // Connection/Session constructed inside the worker lambda (`lambda: sqlite3.connect(...).execute(...)`)
            // runs wholly on the worker thread and does not cross threads.
            match attr.value.as_ref() {
                Expr::Name(n) if !params.contains(n.id.as_str()) => {}
                Expr::Attribute(_) => {}
                _ => return,
            }
        }
        if matches!(
            attr.value.inferred_type(self.model),
            Some(Type::ClassLiteral(_) | Type::SubclassOf(_))
        ) {
            return;
        }
        if !self.expr_is_external_bound_call(callable_expr) {
            return;
        }
        let Some(handle_name) = self.stateful_sync_db_handle_of(&attr.value, callable_expr) else {
            return;
        };
        self.clear_pending_call_args();
        self.push_edge(
            caller,
            CalleeRef::External {
                qualname: format!("offload-stateful-session:{}:{handle_name}", kind.as_str()),
            },
            offload_range,
            CallKind::Call,
            false,
            Some(false),
        );
    }

    fn stateful_sync_db_handle_of(
        &mut self,
        recv: &Expr,
        callable_expr: &Expr,
    ) -> Option<&'static str> {
        if let Some(ty) = recv.inferred_type(self.model) {
            let env = self.model.program_environment();
            if let Some(h) = self.type_to_sync_db_handle(ty, &env) {
                return Some(h);
            }
        }
        let ext = self.external_qualname(callable_expr)?;
        canonical_sync_db_handle_from_method(&ext)
    }

    fn type_to_sync_db_handle(
        &self,
        ty: Type<'db>,
        env: &ty_python_semantic::ProgramEnvironment<'db>,
    ) -> Option<&'static str> {
        if matches!(
            ty,
            Type::ClassLiteral(_) | Type::SubclassOf(_) | Type::GenericAlias(_)
        ) {
            return None;
        }
        if let Some(union) = ty.as_union() {
            let mut matched: Option<&'static str> = None;
            for elem in union.elements(self.model.db()).iter() {
                if matches!(elem, Type::Never) || elem.is_none(self.model.db()) {
                    continue;
                }
                let h = self.type_to_sync_db_handle(*elem, env)?;
                matched = Some(matched.unwrap_or(h));
            }
            return matched;
        }
        let (TypeDefinition::StaticClass(def) | TypeDefinition::DynamicClass(def)) =
            ty.definition(self.model.db(), env)?
        else {
            return None;
        };
        let name = def.name(self.model.db())?;
        let module = module_of_definition(self.model, def)?;
        let qual = format!("{module}.{name}");
        canonical_sync_db_handle(&qual)
    }

    // ── 2. swallowed-cancelled-error ────────────────────────────────────────

    pub(crate) fn maybe_record_swallowed_cancelled_error(&mut self, try_stmt: &ast::StmtTry) {
        let Some(caller) = self.current_fn else {
            return;
        };
        if self.dead_depth > 0 || self.in_lambda || self.off_loop_thread_depth > 0 {
            return;
        }
        let caller_fn = self.function(caller);
        if !caller_fn.is_async || caller_fn.is_sync_route || caller_fn.is_installed_third_party() {
            return;
        }
        if stmt_list_always_raises(&try_stmt.finalbody) {
            return;
        }
        let try_has_suspension = body_has_async_suspension(&try_stmt.body);
        let mut prior_caught_cancelled = false;

        for handler in &try_stmt.handlers {
            let ast::ExceptHandler::ExceptHandler(h) = handler;
            let catch_label = match h.type_.as_deref() {
                None => {
                    let fires = !prior_caught_cancelled && try_has_suspension;
                    prior_caught_cancelled = true;
                    fires.then_some("bare-except")
                }
                Some(ty_expr) if self.expr_catches_cancelled_error_explicitly(ty_expr) => {
                    let fires = !prior_caught_cancelled;
                    prior_caught_cancelled = true;
                    fires.then_some("asyncio.CancelledError")
                }
                Some(ty_expr) if self.expr_catches_base_exception(ty_expr) => {
                    let fires = !prior_caught_cancelled && try_has_suspension;
                    prior_caught_cancelled = true;
                    fires.then_some("BaseException")
                }
                Some(_) => None,
            };
            let Some(label) = catch_label else {
                continue;
            };
            if stmt_list_always_raises(&h.body) || body_uses_uncancel_with_raise(&h.body) {
                continue;
            }
            self.clear_pending_call_args();
            self.push_edge(
                caller,
                CalleeRef::External {
                    qualname: format!("swallowed-cancelled-error:{label}"),
                },
                h.range(),
                CallKind::Call,
                false,
                Some(false),
            );
        }
    }

    fn expr_catches_cancelled_error_explicitly(&mut self, expr: &Expr) -> bool {
        if let Expr::Tuple(t) = expr {
            return t
                .elts
                .iter()
                .any(|e| self.expr_catches_cancelled_error_explicitly(e));
        }
        if let Some(q) = self.external_qualname(expr) {
            return matches!(
                q.as_str(),
                "asyncio.CancelledError" | "asyncio.exceptions.CancelledError"
            );
        }
        false
    }

    fn expr_catches_base_exception(&mut self, expr: &Expr) -> bool {
        if let Expr::Tuple(t) = expr {
            return t.elts.iter().any(|e| self.expr_catches_base_exception(e));
        }
        if let Some(q) = self.external_qualname(expr) {
            return matches!(q.as_str(), "builtins.BaseException" | "BaseException");
        }
        matches!(expr, Expr::Name(n) if n.id.as_str() == "BaseException")
    }
}

fn body_has_async_suspension(stmts: &[Stmt]) -> bool {
    use ruff_python_ast::visitor::{Visitor, walk_expr, walk_stmt};
    struct SuspVisitor(bool);
    impl Visitor<'_> for SuspVisitor {
        fn visit_stmt(&mut self, stmt: &Stmt) {
            if self.0 {
                return;
            }
            match stmt {
                Stmt::FunctionDef(_) | Stmt::ClassDef(_) => {}
                Stmt::For(f) => {
                    if f.is_async {
                        self.0 = true;
                        return;
                    }
                    walk_stmt(self, stmt);
                }
                Stmt::With(w) => {
                    if w.is_async {
                        self.0 = true;
                        return;
                    }
                    walk_stmt(self, stmt);
                }
                _ => walk_stmt(self, stmt),
            }
        }
        fn visit_expr(&mut self, expr: &Expr) {
            if self.0 {
                return;
            }
            match expr {
                Expr::Lambda(_) => {}
                Expr::Await(_) => self.0 = true,
                Expr::ListComp(c) if c.generators.iter().any(|g| g.is_async) => self.0 = true,
                Expr::SetComp(c) if c.generators.iter().any(|g| g.is_async) => self.0 = true,
                Expr::DictComp(c) if c.generators.iter().any(|g| g.is_async) => self.0 = true,
                Expr::Generator(c) if c.generators.iter().any(|g| g.is_async) => self.0 = true,
                _ => walk_expr(self, expr),
            }
        }
    }
    let mut v = SuspVisitor(false);
    for s in stmts {
        v.visit_stmt(s);
    }
    v.0
}

fn stmt_list_always_raises(stmts: &[Stmt]) -> bool {
    for stmt in stmts {
        match stmt {
            Stmt::Raise(_) => return true,
            Stmt::Return(_) | Stmt::Break(_) | Stmt::Continue(_) => return false,
            Stmt::If(s) => {
                let has_else = s.elif_else_clauses.last().is_some_and(|c| c.test.is_none());
                if has_else
                    && stmt_list_always_raises(&s.body)
                    && s.elif_else_clauses
                        .iter()
                        .all(|c| stmt_list_always_raises(&c.body))
                {
                    return true;
                }
                if stmt_can_exit_without_raise(stmt, false) {
                    return false;
                }
            }
            Stmt::With(w) => {
                if stmt_list_always_raises(&w.body) {
                    return true;
                }
                if stmt_can_exit_without_raise(stmt, false) {
                    return false;
                }
            }
            Stmt::Try(t) => {
                if stmt_list_always_raises(&t.finalbody) {
                    return true;
                }
                let try_raises =
                    stmt_list_always_raises(&t.body) || stmt_list_always_raises(&t.orelse);
                let handlers_raise = t.handlers.iter().all(|h| {
                    let ast::ExceptHandler::ExceptHandler(h) = h;
                    stmt_list_always_raises(&h.body)
                });
                if try_raises && handlers_raise {
                    return true;
                }
                if stmt_can_exit_without_raise(stmt, false) {
                    return false;
                }
            }
            _ => {
                if stmt_can_exit_without_raise(stmt, false) {
                    return false;
                }
            }
        }
    }
    false
}

fn stmt_can_exit_without_raise(stmt: &Stmt, in_loop: bool) -> bool {
    match stmt {
        Stmt::FunctionDef(_) | Stmt::ClassDef(_) => false,
        Stmt::Return(_) => true,
        Stmt::Break(_) | Stmt::Continue(_) => !in_loop,
        Stmt::If(s) => {
            s.body
                .iter()
                .any(|st| stmt_can_exit_without_raise(st, in_loop))
                || s.elif_else_clauses.iter().any(|c| {
                    c.body
                        .iter()
                        .any(|st| stmt_can_exit_without_raise(st, in_loop))
                })
        }
        Stmt::For(s) => {
            s.body
                .iter()
                .any(|st| stmt_can_exit_without_raise(st, true))
                || s.orelse
                    .iter()
                    .any(|st| stmt_can_exit_without_raise(st, in_loop))
        }
        Stmt::While(s) => {
            s.body
                .iter()
                .any(|st| stmt_can_exit_without_raise(st, true))
                || s.orelse
                    .iter()
                    .any(|st| stmt_can_exit_without_raise(st, in_loop))
        }
        Stmt::With(w) => w
            .body
            .iter()
            .any(|st| stmt_can_exit_without_raise(st, in_loop)),
        Stmt::Try(t) => {
            t.body
                .iter()
                .any(|st| stmt_can_exit_without_raise(st, in_loop))
                || t.handlers.iter().any(|h| {
                    let ast::ExceptHandler::ExceptHandler(h) = h;
                    h.body
                        .iter()
                        .any(|st| stmt_can_exit_without_raise(st, in_loop))
                })
                || t.orelse
                    .iter()
                    .any(|st| stmt_can_exit_without_raise(st, in_loop))
                || t.finalbody
                    .iter()
                    .any(|st| stmt_can_exit_without_raise(st, in_loop))
        }
        Stmt::Match(m) => m.cases.iter().any(|c| {
            c.body
                .iter()
                .any(|st| stmt_can_exit_without_raise(st, in_loop))
        }),
        _ => false,
    }
}

fn body_uses_uncancel_with_raise(stmts: &[Stmt]) -> bool {
    use ruff_python_ast::visitor::{Visitor, walk_expr, walk_stmt};
    struct UncancelVisitor {
        saw_uncancel: bool,
        saw_raise: bool,
    }
    impl Visitor<'_> for UncancelVisitor {
        fn visit_stmt(&mut self, stmt: &Stmt) {
            match stmt {
                Stmt::FunctionDef(_) | Stmt::ClassDef(_) => {}
                Stmt::Raise(_) => {
                    self.saw_raise = true;
                    walk_stmt(self, stmt);
                }
                _ => walk_stmt(self, stmt),
            }
        }
        fn visit_expr(&mut self, expr: &Expr) {
            match expr {
                Expr::Lambda(_) => {}
                Expr::Call(c) => {
                    if let Expr::Attribute(attr) = c.func.as_ref()
                        && matches!(attr.attr.as_str(), "uncancel" | "cancelling")
                    {
                        self.saw_uncancel = true;
                    }
                    walk_expr(self, expr);
                }
                _ => walk_expr(self, expr),
            }
        }
    }
    let mut v = UncancelVisitor {
        saw_uncancel: false,
        saw_raise: false,
    };
    for s in stmts {
        v.visit_stmt(s);
    }
    v.saw_uncancel && v.saw_raise
}
