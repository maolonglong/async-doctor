//! `useless-local-async-lock` seed detection for `CallCollector`.

use std::collections::{HashMap, HashSet};

use async_doctor_core::{CallKind, CalleeRef, FunctionId};
use ruff_python_ast::visitor::{
    Visitor, walk_arguments, walk_except_handler, walk_expr, walk_stmt,
};
use ruff_python_ast::{self as ast, Expr, Stmt};
use ruff_text_size::Ranged;

use super::super::CallCollector;
use crate::extract::constants::body_has_yield;

fn canonical_async_lock_ctor(qualname: &str) -> Option<&'static str> {
    match qualname {
        "asyncio.Lock" | "asyncio.locks.Lock" => Some("asyncio.Lock"),
        "asyncio.Semaphore" | "asyncio.locks.Semaphore" => Some("asyncio.Semaphore"),
        "asyncio.BoundedSemaphore" | "asyncio.locks.BoundedSemaphore" => {
            Some("asyncio.BoundedSemaphore")
        }
        "anyio.Lock" => Some("anyio.Lock"),
        "anyio.Semaphore" => Some("anyio.Semaphore"),
        "anyio.CapacityLimiter" => Some("anyio.CapacityLimiter"),
        _ => None,
    }
}

impl<'a, 'db> CallCollector<'a, 'db> {
    pub(crate) fn fresh_async_lock_kind(&mut self, call: &ast::ExprCall) -> Option<&'static str> {
        let q = self.external_qualname(&call.func)?;
        canonical_async_lock_ctor(&q)
    }

    /// Pre-compute function-local `Lock` / `Semaphore` bindings inside `func`
    /// that never escape to nested closures, call arguments, attributes, or returns.
    pub(crate) fn compute_unshared_local_async_locks(
        &mut self,
        fid: FunctionId,
        func: &ast::StmtFunctionDef,
    ) -> HashMap<String, &'static str> {
        let f = self.function(fid);
        if !func.is_async
            || f.is_generator
            || f.is_installed_third_party()
            || body_has_yield(&func.body)
        {
            return HashMap::new();
        }

        let mut scanner = LockEscapeScanner {
            collector: self,
            in_nested_scope: false,
            lock_bindings: HashMap::new(),
            binding_counts: HashMap::new(),
            escaped_names: HashSet::new(),
            saw_frame_introspection: false,
        };
        scanner.visit_body(&func.body);
        if scanner.saw_frame_introspection {
            return HashMap::new();
        }

        let LockEscapeScanner {
            lock_bindings,
            binding_counts,
            escaped_names,
            ..
        } = scanner;

        let mut out = HashMap::new();
        for (name, kind) in lock_bindings {
            if binding_counts.get(&name).copied().unwrap_or(0) == 1
                && !escaped_names.contains(&name)
                && !self.current_param_names.iter().any(|p| p == &name)
                && !self.nonlocal_or_global_names.contains(&name)
            {
                out.insert(name, kind);
            }
        }
        out
    }

    pub(crate) fn maybe_record_useless_local_async_lock(
        &mut self,
        caller: FunctionId,
        item: &ast::WithItem,
    ) {
        if self.dead_depth > 0 || self.in_lambda || self.off_loop_thread_depth > 0 {
            return;
        }
        let f = self.function(caller);
        if !f.is_async || f.is_generator || f.is_installed_third_party() {
            return;
        }
        let lock_kind = match &item.context_expr {
            Expr::Call(call) => {
                let Some(kind) = self.fresh_async_lock_kind(call) else {
                    return;
                };
                match item.optional_vars.as_deref() {
                    None => kind,
                    Some(Expr::Name(n))
                        if self.unshared_local_locks.contains_key(n.id.as_str()) =>
                    {
                        kind
                    }
                    _ => return,
                }
            }
            Expr::Name(n) if item.optional_vars.is_none() => {
                let Some(&kind) = self.unshared_local_locks.get(n.id.as_str()) else {
                    return;
                };
                kind
            }
            _ => return,
        };
        self.clear_pending_call_args();
        self.push_edge(
            caller,
            CalleeRef::External {
                qualname: format!("useless-local-async-lock:{lock_kind}"),
            },
            item.context_expr.range(),
            CallKind::WithEnter,
            false,
            Some(false),
        );
    }
}

struct LockEscapeScanner<'c, 'a, 'db> {
    collector: &'c mut CallCollector<'a, 'db>,
    in_nested_scope: bool,
    lock_bindings: HashMap<String, &'static str>,
    binding_counts: HashMap<String, u32>,
    escaped_names: HashSet<String>,
    saw_frame_introspection: bool,
}

impl LockEscapeScanner<'_, '_, '_> {
    fn note_target(&mut self, target: &Expr) {
        match target {
            Expr::Name(n) => {
                *self
                    .binding_counts
                    .entry(n.id.as_str().to_string())
                    .or_default() += 1;
            }
            Expr::Tuple(t) => {
                for e in &t.elts {
                    self.note_target(e);
                }
            }
            Expr::List(l) => {
                for e in &l.elts {
                    self.note_target(e);
                }
            }
            Expr::Starred(s) => self.note_target(&s.value),
            other => self.visit_expr(other),
        }
    }
}

impl Visitor<'_> for LockEscapeScanner<'_, '_, '_> {
    fn visit_stmt(&mut self, stmt: &Stmt) {
        match stmt {
            Stmt::FunctionDef(f) => {
                *self
                    .binding_counts
                    .entry(f.name.as_str().to_string())
                    .or_default() += 1;
                for d in &f.decorator_list {
                    self.visit_expr(&d.expression);
                }
                let prev = std::mem::replace(&mut self.in_nested_scope, true);
                self.visit_body(&f.body);
                self.in_nested_scope = prev;
            }
            Stmt::ClassDef(c) => {
                *self
                    .binding_counts
                    .entry(c.name.as_str().to_string())
                    .or_default() += 1;
                for d in &c.decorator_list {
                    self.visit_expr(&d.expression);
                }
                let prev = std::mem::replace(&mut self.in_nested_scope, true);
                self.visit_body(&c.body);
                self.in_nested_scope = prev;
            }
            Stmt::Assign(a) => {
                if !self.in_nested_scope
                    && a.targets.len() == 1
                    && let Expr::Name(n) = &a.targets[0]
                {
                    *self
                        .binding_counts
                        .entry(n.id.as_str().to_string())
                        .or_default() += 1;
                    if let Expr::Call(call) = a.value.as_ref()
                        && let Some(kind) = self.collector.fresh_async_lock_kind(call)
                    {
                        self.lock_bindings.insert(n.id.as_str().to_string(), kind);
                        walk_arguments(self, &call.arguments);
                        return;
                    }
                    self.visit_expr(&a.value);
                    return;
                }
                for t in &a.targets {
                    self.note_target(t);
                }
                self.visit_expr(&a.value);
            }
            Stmt::AnnAssign(ann) => {
                if !self.in_nested_scope
                    && let Expr::Name(n) = ann.target.as_ref()
                    && let Some(val) = ann.value.as_deref()
                {
                    *self
                        .binding_counts
                        .entry(n.id.as_str().to_string())
                        .or_default() += 1;
                    if let Expr::Call(call) = val
                        && let Some(kind) = self.collector.fresh_async_lock_kind(call)
                    {
                        self.lock_bindings.insert(n.id.as_str().to_string(), kind);
                        walk_arguments(self, &call.arguments);
                        return;
                    }
                    self.visit_expr(val);
                    return;
                }
                self.note_target(&ann.target);
                if let Some(val) = ann.value.as_deref() {
                    self.visit_expr(val);
                }
            }
            Stmt::AugAssign(aug) => {
                self.note_target(&aug.target);
                self.visit_expr(&aug.value);
            }
            Stmt::For(f) => {
                self.note_target(&f.target);
                self.visit_expr(&f.iter);
                self.visit_body(&f.body);
                self.visit_body(&f.orelse);
            }
            Stmt::With(w) => {
                for item in &w.items {
                    if !self.in_nested_scope
                        && w.is_async
                        && matches!(&item.context_expr, Expr::Name(_))
                        && item.optional_vars.is_none()
                    {
                        continue;
                    }
                    if !self.in_nested_scope
                        && w.is_async
                        && let (Some(Expr::Name(n)), Expr::Call(call)) =
                            (item.optional_vars.as_deref(), &item.context_expr)
                        && let Some(kind) = self.collector.fresh_async_lock_kind(call)
                    {
                        *self
                            .binding_counts
                            .entry(n.id.as_str().to_string())
                            .or_default() += 1;
                        self.lock_bindings.insert(n.id.as_str().to_string(), kind);
                        walk_arguments(self, &call.arguments);
                        continue;
                    }
                    self.visit_expr(&item.context_expr);
                    if let Some(vars) = item.optional_vars.as_deref() {
                        self.note_target(vars);
                    }
                }
                self.visit_body(&w.body);
            }
            _ => walk_stmt(self, stmt),
        }
    }

    fn visit_except_handler(&mut self, except_handler: &ast::ExceptHandler) {
        let ast::ExceptHandler::ExceptHandler(h) = except_handler;
        if let Some(name) = &h.name {
            *self
                .binding_counts
                .entry(name.as_str().to_string())
                .or_default() += 1;
        }
        walk_except_handler(self, except_handler);
    }

    fn visit_expr(&mut self, expr: &Expr) {
        match expr {
            Expr::Name(n) => {
                self.escaped_names.insert(n.id.as_str().to_string());
            }
            Expr::Lambda(lam) => {
                let prev = std::mem::replace(&mut self.in_nested_scope, true);
                walk_expr(self, &Expr::Lambda(lam.clone()));
                self.in_nested_scope = prev;
            }
            Expr::Call(c) => {
                if let Expr::Name(n) = c.func.as_ref()
                    && matches!(n.id.as_str(), "locals" | "vars" | "eval")
                {
                    self.saw_frame_introspection = true;
                }
                walk_expr(self, expr);
            }
            _ => walk_expr(self, expr),
        }
    }
}
