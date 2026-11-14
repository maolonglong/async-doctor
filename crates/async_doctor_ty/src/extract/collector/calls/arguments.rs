//! Argument resolution, property access, imperative routes, and task spawns.

use async_doctor_core::{CallKind, CalleeRef, FunctionId};
use ruff_python_ast::{self as ast, Expr};
use ruff_text_size::Ranged;
use ty_python_semantic::definitions_for_attribute;

use super::CallCollector;
use crate::extract::helpers::{
    is_partial_call, is_task_spawn_callee, route_handler_arg, twin_relevant_client_root,
};

impl<'a, 'db> CallCollector<'a, 'db> {
    pub(crate) fn clear_pending_call_args(&mut self) {
        self.pending_literals.clear();
        self.pending_keyword_literals.clear();
        self.pending_callables.clear();
        self.pending_externals.clear();
        self.pending_constructed_receivers.clear();
        self.pending_constructed_receiver = None;
        self.pending_keyword_callables.clear();
        self.pending_is_bound_call = false;
    }

    pub(crate) fn note_context_binding(&mut self, name: &str, value: &'a Expr) {
        let is_ctx = match value {
            Expr::Call(c) => self.is_copy_context_or_context_ctor(c),
            Expr::Name(src) => self.context_locals.contains(src.id.as_str()),
            _ => false,
        };
        if is_ctx {
            self.context_locals.insert(name.to_string());
        } else {
            self.context_locals.remove(name);
        }
    }

    pub(crate) fn note_partial_binding(&mut self, name: &str, value: &'a Expr) {
        let unwrapped = match value {
            Expr::Call(c) if self.is_partial_call_ctx(c) => self.unwrap_callable_expr(value),
            Expr::Name(src) => self.local_partials.get(src.id.as_str()).copied(),
            _ => None,
        };
        if let Some(target) = unwrapped {
            self.local_partials.insert(name.to_string(), target);
        } else {
            self.local_partials.remove(name);
        }
    }

    pub(crate) fn is_copy_context_or_context_ctor(&mut self, call: &ast::ExprCall) -> bool {
        if self.resolve_project_function(&call.func).is_some()
            || !self.resolve_project_constructors(&call.func).is_empty()
        {
            return false;
        }
        if let Some(q) = self.external_qualname(&call.func) {
            return matches!(
                q.as_str(),
                "contextvars.copy_context"
                    | "_contextvars.copy_context"
                    | "contextvars.Context"
                    | "_contextvars.Context"
            );
        }
        match call.func.as_ref() {
            Expr::Name(n) => n.id.as_str() == "copy_context",
            Expr::Attribute(a) => {
                matches!(a.attr.as_str(), "copy_context" | "Context")
                    && matches!(
                        a.value.as_ref(),
                        Expr::Name(n) if matches!(n.id.as_str(), "contextvars" | "_contextvars")
                    )
            }
            _ => false,
        }
    }

    pub(crate) fn is_context_run_callable(&mut self, expr: &Expr) -> bool {
        let Expr::Attribute(attr) = expr else {
            return false;
        };
        if attr.attr.as_str() != "run" || self.resolve_project_function(expr).is_some() {
            return false;
        }
        if let Some(q) = self.external_qualname(expr)
            && matches!(
                q.as_str(),
                "contextvars.Context.run" | "_contextvars.Context.run"
            )
        {
            return true;
        }
        if let Some(ty) = self.type_instance_qualname(&attr.value)
            && matches!(ty.as_str(), "contextvars.Context" | "_contextvars.Context")
        {
            return true;
        }
        match attr.value.as_ref() {
            Expr::Name(n) => self.context_locals.contains(n.id.as_str()),
            Expr::Call(c) => self.is_copy_context_or_context_ctor(c),
            _ => false,
        }
    }

    pub(crate) fn is_partial_call_ctx(&mut self, call: &ast::ExprCall) -> bool {
        if self.resolve_project_function(&call.func).is_some()
            || !self.resolve_project_constructors(&call.func).is_empty()
        {
            return false;
        }
        if let Some(q) = self.external_qualname(&call.func) {
            return matches!(q.as_str(), "functools.partial" | "_functools.partial");
        }
        is_partial_call(call)
    }

    /// Unwrap `functools.partial(...)` (including `partial(ctx.run, fn, ...)`)
    /// and `local_partials` aliases down to the underlying callable expression.
    pub(crate) fn unwrap_callable_expr(&mut self, mut expr: &'a Expr) -> Option<&'a Expr> {
        for _ in 0..8 {
            match expr {
                Expr::Name(n) => {
                    if let Some(inner) = self.local_partials.get(n.id.as_str()).copied() {
                        expr = inner;
                        continue;
                    }
                    return Some(expr);
                }
                Expr::Call(c) if self.is_partial_call_ctx(c) => {
                    let first = c.arguments.args.first()?;
                    expr = if self.is_context_run_callable(first) {
                        c.arguments.args.get(1)?
                    } else {
                        first
                    };
                }
                _ => return Some(expr),
            }
        }
        Some(expr)
    }

    /// Resolve the effective offloaded callable at `callable_idx`, shifting by +1
    /// when `args[callable_idx]` is `contextvars.Context.run` / `copy_context().run`.
    pub(crate) fn effective_offload_callable(
        &mut self,
        args: &'a [Expr],
        callable_idx: usize,
    ) -> Option<&'a Expr> {
        let first = args.get(callable_idx)?;
        let raw = if self.is_context_run_callable(first) {
            args.get(callable_idx + 1)?
        } else {
            first
        };
        self.unwrap_callable_expr(raw)
    }

    /// Inspect an inline or locally-bound lambda expression: returns `Some(lam)` when
    /// `expr` is `Expr::Lambda` or a `Name` bound in `local_lambdas`.
    pub(crate) fn resolve_lambda_expr(&self, expr: &'a Expr) -> Option<&'a ast::ExprLambda> {
        match expr {
            Expr::Lambda(lam) => Some(lam),
            Expr::Name(n) => self.local_lambdas.get(n.id.as_str()).copied(),
            _ => None,
        }
    }

    /// Resolve callable `FunctionId` and coroutine status for a lambda expression.
    ///
    /// Keeps synthetic `<lambda@...>` `FunctionFact::is_async == false` so
    /// `report.async_entry_count` is never inflated, while returning `(async_fid, true)`
    /// when the lambda's body call returns a proven coroutine.
    pub(crate) fn resolve_lambda_callable_and_coro(
        &mut self,
        lam: &'a ast::ExprLambda,
    ) -> (Option<FunctionId>, bool) {
        let lam_fid = self.ensure_lambda_fact(lam);
        if let Expr::Call(body_call) = lam.body.as_ref()
            && self.call_returns_proven_coroutine(body_call)
        {
            let async_fid = self
                .resolve_project_function(&body_call.func)
                .filter(|&id| self.function(id).is_async);
            return (async_fid, true);
        }
        (Some(lam_fid), false)
    }

    pub(crate) fn resolve_callable_project_fid(&mut self, expr: &'a Expr) -> Option<FunctionId> {
        let target = self.unwrap_callable_expr(expr)?;
        if let Some(lam) = self.resolve_lambda_expr(target) {
            return self.resolve_lambda_callable_and_coro(lam).0;
        }
        match target {
            Expr::Name(_) | Expr::Attribute(_) => self
                .resolve_project_function(target)
                .or_else(|| self.resolve_project_constructors(target).into_iter().next()),
            _ => None,
        }
    }

    pub(crate) fn resolve_keyword_callables(
        &mut self,
        call: &'a ast::ExprCall,
    ) -> Vec<(String, FunctionId)> {
        let mut out = Vec::new();
        for kw in &call.arguments.keywords {
            let Some(name) = kw.arg.as_ref() else {
                continue;
            };
            if let Some(fid) = self.resolve_callable_project_fid(&kw.value) {
                out.push((name.as_str().to_string(), fid));
            }
        }
        out
    }

    pub(crate) fn maybe_property_get(&mut self, attr: &ast::ExprAttribute) {
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
        if !self.property_defs.contains(&fid) {
            return;
        }
        self.clear_pending_call_args();
        self.pending_is_bound_call = true;
        self.push_edge(
            caller,
            CalleeRef::Project(fid),
            attr.range(),
            CallKind::PropertyGet,
            false,
            Some(self.function(fid).is_async),
        );
    }

    pub(crate) fn note_imperative_route(&mut self, call: &ast::ExprCall) {
        let Some(handler) = route_handler_arg(call) else {
            return;
        };
        let Some(fid) = self.resolve_project_function(handler) else {
            return;
        };
        self.mark_imperative_route(fid);
    }

    pub(crate) fn note_task_spawn(&mut self, call: &ast::ExprCall) {
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

    pub(crate) fn resolve_positional_callables(
        &mut self,
        args: &'a [Expr],
    ) -> Vec<Option<FunctionId>> {
        let mut out = Vec::with_capacity(args.len());
        for arg in args {
            out.push(self.resolve_callable_project_fid(arg));
        }
        out
    }

    /// External qualnames for positional args (offload of typed third-party methods).
    pub(crate) fn resolve_positional_externals(&mut self, args: &'a [Expr]) -> Vec<Option<String>> {
        let mut out = Vec::with_capacity(args.len());
        for arg in args {
            let Some(target) = self.unwrap_callable_expr(arg) else {
                out.push(None);
                continue;
            };
            if self.resolve_lambda_expr(target).is_some()
                || self.resolve_project_function(target).is_some()
                || !self.resolve_project_constructors(target).is_empty()
            {
                out.push(None);
                continue;
            }
            out.push(self.external_qualname(target));
        }
        out
    }

    /// Root constructed client class for positional callable args (Azure vs OpenAI).
    pub(crate) fn resolve_positional_constructed_receivers(
        &mut self,
        args: &'a [Expr],
    ) -> Vec<Option<String>> {
        let mut out = Vec::with_capacity(args.len());
        for arg in args {
            let Some(target) = self.unwrap_callable_expr(arg) else {
                out.push(None);
                continue;
            };
            if self.resolve_lambda_expr(target).is_some() {
                out.push(None);
                continue;
            }
            out.push(self.constructed_root_receiver(target));
        }
        out
    }

    /// Walk an attribute chain to the root expression and name its constructed type
    /// when it is a known sync client that affects async-twin selection.
    pub(crate) fn constructed_root_receiver(&mut self, expr: &Expr) -> Option<String> {
        let mut cur = expr;
        while let Expr::Attribute(attr) = cur {
            cur = attr.value.as_ref();
        }
        let ty = self.type_instance_qualname(cur)?;
        twin_relevant_client_root(&ty)
    }
}
