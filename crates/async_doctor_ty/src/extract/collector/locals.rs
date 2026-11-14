//! Local variable binding tracking and inline/synthetic lambda helpers for [`super::CallCollector`].

use async_doctor_core::{EntryLabel, FunctionFact, FunctionId};
use ruff_python_ast::visitor::Visitor;
use ruff_python_ast::{self as ast, Expr};

use super::CallCollector;
use crate::extract::ctx::DefKey;

impl<'a, 'db> CallCollector<'a, 'db> {
    pub(crate) fn note_local_assign(&mut self, assign: &'a ast::StmtAssign) {
        if assign.targets.len() == 1
            && let Expr::Name(n) = &assign.targets[0]
        {
            self.note_local_binding(n.id.as_str(), &assign.value);
            return;
        }
        // Unpacked or chained targets (`sock, addr = srv.accept()`, `a = b = x`)
        // invalidate stale proven-safe local bindings on every bound name.
        for target in &assign.targets {
            self.invalidate_unpacked_target(target);
        }
    }

    fn invalidate_unpacked_target(&mut self, target: &Expr) {
        match target {
            Expr::Name(n) => {
                let name = n.id.as_str();
                self.invalidate_future_done_receiver(name);
                self.async_with_receivers.remove(name);
                self.narrowed_locals.remove(name);
                self.local_lambdas.remove(name);
                self.local_genexprs.remove(name);
                self.idle_loop_locals.remove(name);
                self.unbounded_queues.remove(name);
                self.boto_receivers.remove(name);
                self.boto_session_receivers.remove(name);
                self.aio_boto_receivers.remove(name);
                self.running_loop_locals.remove(name);
                self.event_loop_locals.remove(name);
                self.buffer_locals.remove(name);
                self.in_memory_archive_receivers.remove(name);
                self.local_ephemeral_clients.remove(name);
                self.local_stateful_sessions.remove(name);
                self.in_memory_sqlite_vars.remove(name);
                self.nonblocking_socket_vars.remove(name);
                self.local_partials.remove(name);
                self.context_locals.remove(name);
            }
            Expr::Tuple(t) => {
                for elt in &t.elts {
                    self.invalidate_unpacked_target(elt);
                }
            }
            Expr::List(l) => {
                for elt in &l.elts {
                    self.invalidate_unpacked_target(elt);
                }
            }
            Expr::Starred(s) => self.invalidate_unpacked_target(&s.value),
            _ => {}
        }
    }

    pub(crate) fn note_local_binding(&mut self, name: &str, value: &'a Expr) {
        // Rebind invalidates `.done()` proofs, branch narrowings, and prior
        // `async with ... as name` receiver types for this local.
        self.invalidate_future_done_receiver(name);
        self.async_with_receivers.remove(name);
        self.narrowed_locals.remove(name);
        match value {
            Expr::Lambda(lam) => {
                self.local_lambdas.insert(name.to_string(), lam);
                self.idle_loop_locals.remove(name);
                self.unbounded_queues.remove(name);
                self.boto_receivers.remove(name);
                self.boto_session_receivers.remove(name);
                self.aio_boto_receivers.remove(name);
                self.running_loop_locals.remove(name);
                self.event_loop_locals.remove(name);
                self.buffer_locals.remove(name);
                self.in_memory_archive_receivers.remove(name);
                self.local_ephemeral_clients.remove(name);
                self.local_stateful_sessions.remove(name);
                self.in_memory_sqlite_vars.remove(name);
                self.nonblocking_socket_vars.remove(name);
                self.local_partials.remove(name);
                self.context_locals.remove(name);
            }
            Expr::Generator(gexpr) => {
                self.local_genexprs
                    .entry(name.to_string())
                    .or_default()
                    .push(gexpr);
                self.idle_loop_locals.remove(name);
                self.unbounded_queues.remove(name);
                self.boto_receivers.remove(name);
                self.boto_session_receivers.remove(name);
                self.aio_boto_receivers.remove(name);
                self.running_loop_locals.remove(name);
                self.event_loop_locals.remove(name);
                self.buffer_locals.remove(name);
                self.in_memory_archive_receivers.remove(name);
                self.local_ephemeral_clients.remove(name);
                self.local_stateful_sessions.remove(name);
                self.in_memory_sqlite_vars.remove(name);
                self.nonblocking_socket_vars.remove(name);
                self.local_partials.remove(name);
                self.context_locals.remove(name);
            }
            other => {
                self.local_lambdas.remove(name);
                self.local_genexprs.remove(name);
                if super::loop_guards::value_is_idle_loop_binding(other, &self.idle_loop_helpers) {
                    self.idle_loop_locals.insert(name.to_string());
                } else {
                    self.idle_loop_locals.remove(name);
                }
                self.note_queue_binding(name, other);
                self.note_boto_binding(name, other);
                self.note_loop_handle_binding(name, other);
                self.note_buffer_binding(name, other);
                self.note_archive_binding(name, other);
                self.note_ephemeral_client_binding(name, other);
                self.note_stateful_session_binding(name, other);
                self.note_sqlite_binding(name, other);
                self.note_socket_binding(name, other);
                self.note_context_binding(name, other);
                self.note_partial_binding(name, other);
            }
        }
    }

    fn invalidate_future_done_receiver(&mut self, name: &str) {
        let prefix = format!("{name}.");
        self.future_done_receivers
            .retain(|p| p != name && !p.starts_with(&prefix));
    }

    fn note_loop_handle_binding(&mut self, name: &str, value: &Expr) {
        use super::loop_guards::{expr_is_get_event_loop, expr_is_get_running_loop};
        if expr_is_get_running_loop(value) {
            self.running_loop_locals.insert(name.to_string());
            self.event_loop_locals.remove(name);
            return;
        }
        if expr_is_get_event_loop(value) {
            self.event_loop_locals.insert(name.to_string());
            self.running_loop_locals.remove(name);
            return;
        }
        // `except RuntimeError: loop = None` keeps get_running_loop provenance.
        if matches!(value, Expr::NoneLiteral(_)) {
            return;
        }
        self.running_loop_locals.remove(name);
        self.event_loop_locals.remove(name);
    }

    /// Analyze a local lambda called directly at the current call site (`f()`).
    ///
    /// Unlike [`Self::ensure_lambda_fact`] (which emits edges from a separate
    /// synthetic `FunctionFact`), inline expansion emits edges directly from
    /// `self.current_fn`, so the call site's enclosing branch/loop/no-loop
    /// guards must remain active.
    pub(crate) fn analyze_lambda_body(&mut self, lambda: &'a ast::ExprLambda) {
        let prev_in = self.in_lambda;
        self.in_lambda = false;
        self.visit_expr(&lambda.body);
        self.in_lambda = prev_in;
    }

    pub(crate) fn ensure_lambda_fact(&mut self, lam: &'a ast::ExprLambda) -> FunctionId {
        let key = DefKey {
            file: self.ctx.rel.clone(),
            name_start: u32::from(lam.range.start()),
        };
        if let Some(id) = self.def_get(&key) {
            return id;
        }
        let id = self.next_function_id();
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
            end_line: self.ctx.line_of(lam.range.end()),
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
            offload_decorator: None,
        });
        // Analyze lambda body as edges from the synthetic function.
        // Judge body edges relative to the lambda itself — enclosing try/if/loop
        // tags on the offload site must not make lambda sinks look conditional.
        let prev = self.current_fn;
        let prev_params = std::mem::take(&mut self.current_param_names);
        let prev_in = self.in_lambda;
        let depths = self.take_enclosing_depths();
        self.current_fn = Some(id);
        self.current_param_names = self.function(id).param_names.clone();
        self.in_lambda = false;
        self.visit_expr(&lam.body);
        self.in_lambda = prev_in;
        self.current_fn = prev;
        self.current_param_names = prev_params;
        self.restore_enclosing_depths(depths);
        id
    }

    /// Snapshot + clear branch/loop depths so nested callable bodies are graded
    /// relative to themselves (not the enclosing offload site's try/if/loop).
    pub(super) fn take_enclosing_depths(&mut self) -> (u32, u32, u32, u32, u32, u32) {
        let depths = (
            self.dead_depth,
            self.loop_depth,
            self.branch_depth,
            self.cache_miss_depth,
            self.no_running_loop_depth,
            self.off_loop_thread_depth,
        );
        self.dead_depth = 0;
        self.loop_depth = 0;
        self.branch_depth = 0;
        self.cache_miss_depth = 0;
        self.no_running_loop_depth = 0;
        self.off_loop_thread_depth = 0;
        depths
    }

    pub(super) fn restore_enclosing_depths(&mut self, depths: (u32, u32, u32, u32, u32, u32)) {
        self.dead_depth = depths.0;
        self.loop_depth = depths.1;
        self.branch_depth = depths.2;
        self.cache_miss_depth = depths.3;
        self.no_running_loop_depth = depths.4;
        self.off_loop_thread_depth = depths.5;
    }
}
