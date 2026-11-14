//! Call / await / property / lambda / task edge recording.

mod arguments;
mod callbacks;

use async_doctor_core::{CallKind, CalleeRef, OffloadKind};
use ruff_db::files::FilePath;
use ruff_python_ast::visitor::Visitor;
use ruff_python_ast::{self as ast, Expr};
use ruff_text_size::Ranged;
use ty_python_semantic::{ImportAliasResolution, definitions_for_attribute, definitions_for_name};

use super::CallCollector;
use crate::extract::constants::is_generator_consumer;
use crate::extract::helpers::{
    is_asyncio_redis_type, is_callable_ref, is_getattr_call, is_path_or_file_open_qual,
    normalize_asyncio_redis_type, qualname_from_definition,
};

impl<'a, 'db> CallCollector<'a, 'db> {
    pub(crate) fn walk_call_children(
        &mut self,
        call: &'a ast::ExprCall,
        offload_known: Option<Option<OffloadKind>>,
    ) {
        let offload_kind = match offload_known {
            Some(k) => k,
            None => self.is_offload_callee(&call.func),
        };
        let consumer = is_generator_consumer(&call.func);
        self.in_callee_position = true;
        self.visit_expr(&call.func);
        self.in_callee_position = false;

        if let Some(kind) = offload_kind {
            let callable_idx = kind.callable_arg_index();
            let shifts_via_ctx_run = call
                .arguments
                .args
                .get(callable_idx)
                .is_some_and(|arg| self.is_context_run_callable(arg));
            let actual_callable_idx = if shifts_via_ctx_run {
                callable_idx + 1
            } else {
                callable_idx
            };
            for (i, arg) in call.arguments.args.iter().enumerate() {
                if shifts_via_ctx_run && i == callable_idx {
                    if let Expr::Attribute(attr) = arg
                        && !matches!(attr.value.as_ref(), Expr::Name(_))
                    {
                        self.visit_eager_offload_expr(&attr.value, consumer);
                    }
                    continue;
                }
                if i == actual_callable_idx {
                    if let Expr::Call(inner) = arg
                        && self.is_partial_call_ctx(inner)
                    {
                        self.visit_partial_eager_args(inner, consumer);
                        continue;
                    }
                    if is_callable_ref(arg) {
                        continue;
                    }
                }
                // Non-callable (or non-target) args of an offload are evaluated on
                // the event loop before the offload runs — mark eager.
                self.visit_eager_offload_expr(arg, consumer);
            }
            for kw in &call.arguments.keywords {
                if let Expr::Call(inner) = &kw.value
                    && self.is_partial_call_ctx(inner)
                {
                    self.visit_partial_eager_args(inner, consumer);
                    continue;
                }
                let prev_eager = self.in_eager_offload_arg;
                if !is_callable_ref(&kw.value) {
                    self.in_eager_offload_arg = true;
                }
                self.visit_expr(&kw.value);
                self.in_eager_offload_arg = prev_eager;
            }
        } else if consumer {
            if call.arguments.args.len() == 1
                && call.arguments.keywords.is_empty()
                && self.is_exhaustive_generator_consumer(&call.func)
            {
                self.consume_named_generator(&call.arguments.args[0], true);
            }
            self.consuming_generator = true;
            for arg in &call.arguments.args {
                self.visit_expr(arg);
            }
            for kw in &call.arguments.keywords {
                self.visit_expr(&kw.value);
            }
            self.consuming_generator = false;
        } else {
            for arg in &call.arguments.args {
                self.visit_expr(arg);
            }
            for kw in &call.arguments.keywords {
                self.visit_expr(&kw.value);
            }
        }
    }

    fn visit_eager_offload_expr(&mut self, expr: &'a Expr, consumer: bool) {
        let prev_eager = self.in_eager_offload_arg;
        self.in_eager_offload_arg = true;
        if consumer {
            self.consuming_generator = true;
        }
        self.visit_expr(expr);
        if consumer {
            self.consuming_generator = false;
        }
        self.in_eager_offload_arg = prev_eager;
    }

    /// `functools.partial(fn, a, k=b)` (or `partial(ctx.run, fn, a, k=b)`) passed
    /// to an offload defers the callable while evaluating bound args eagerly.
    fn visit_partial_eager_args(&mut self, inner: &'a ast::ExprCall, consumer: bool) {
        let starts_with_ctx_run = inner
            .arguments
            .args
            .first()
            .is_some_and(|a| self.is_context_run_callable(a));
        let target_idx = usize::from(starts_with_ctx_run);
        if starts_with_ctx_run
            && let Some(Expr::Attribute(attr)) = inner.arguments.args.first()
            && !matches!(attr.value.as_ref(), Expr::Name(_))
        {
            self.visit_eager_offload_expr(&attr.value, consumer);
        }
        if let Some(Expr::Call(nested)) = inner.arguments.args.get(target_idx)
            && self.is_partial_call_ctx(nested)
        {
            self.visit_partial_eager_args(nested, consumer);
        }
        for eager_arg in inner.arguments.args.iter().skip(target_idx + 1) {
            self.visit_eager_offload_expr(eager_arg, consumer);
        }
        for kw in &inner.arguments.keywords {
            self.visit_eager_offload_expr(&kw.value, consumer);
        }
    }

    /// Bind `async with <asyncio redis pipeline()> as name` for receiver-type lookups.
    pub(crate) fn note_async_with_receiver(&mut self, item: &ast::WithItem) {
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

    pub(crate) fn note_async_for_receivers(&mut self, for_stmt: &ast::StmtFor) {
        // `async for k in client.scan_iter(...)`: bind the iterator receiver when it
        // is an asyncio-redis client so later uses of bound names stay async-keyed.
        // The interesting type is on the method receiver (`client`), not the loop var.
        let _ = for_stmt.target;
        if let Some(recv) = self.asyncio_redis_iter_receiver(&for_stmt.iter)
            && let Expr::Call(call) = for_stmt.iter.as_ref()
            && let Expr::Attribute(attr) = call.func.as_ref()
            && let Expr::Name(n) = attr.value.as_ref()
        {
            self.async_with_receivers
                .insert(n.id.as_str().to_string(), recv);
        }
    }

    /// Resolve an asyncio-redis client/Pipeline type for `r.pipeline()` contexts.
    pub(crate) fn asyncio_redis_pipeline_receiver(&mut self, expr: &Expr) -> Option<String> {
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
    pub(crate) fn asyncio_redis_iter_receiver(&self, expr: &Expr) -> Option<String> {
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
    pub(crate) fn expr_looks_like_asyncio_redis(&self, expr: &Expr) -> bool {
        if let Some(ty) = self.type_instance_qualname(expr)
            && is_asyncio_redis_type(&ty)
        {
            return true;
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

    /// Handle a call expression and return the resolved offload kind if any
    /// (`None` if skipped before callee classification).
    pub(crate) fn handle_call(
        &mut self,
        call: &'a ast::ExprCall,
        is_awaited: bool,
    ) -> Option<Option<OffloadKind>> {
        self.pending_is_awaited = is_awaited;
        // Module-level and function-level: task spawns + imperative routes.
        self.note_task_spawn(call);
        self.note_imperative_route(call);

        let caller = self.current_fn?;
        if self.dead_depth > 0 || self.in_lambda || self.off_loop_thread_depth > 0 {
            return None;
        }

        // Lambda assigned to a local then called: `f = lambda: ...; f()`.
        if let Expr::Name(name) = call.func.as_ref()
            && let Some(lambda) = self.local_lambdas.get(name.id.as_str()).copied()
        {
            self.analyze_lambda_body(lambda);
        }

        // `if fut.done(): fut.result()` — result does not block.
        if self.call_is_done_guarded_future_result(call) {
            self.clear_pending_call_args();
            self.pending_is_awaited = false;
            return Some(None);
        }

        self.maybe_record_asyncio_callback(caller, call);
        self.pending_is_awaited = is_awaited;
        self.pending_literals = call
            .arguments
            .args
            .iter()
            .map(|a| self.call_arg_literal(a))
            .collect();
        self.pending_keyword_literals = self.call_keyword_literals(&call.arguments.keywords);
        self.pending_callables = self.resolve_positional_callables(call.arguments.args.as_ref());
        self.pending_externals.clear();
        self.pending_constructed_receivers.clear();
        self.pending_constructed_receiver = None;
        self.pending_keyword_callables = self.resolve_keyword_callables(call);
        self.maybe_record_local_client_close(caller, call);

        // Fast path for first-party project function calls: skips external qualname
        // and stdlib buffer/archive/lock/offload checks.
        if let Some(fid) = self.resolve_project_function(&call.func) {
            self.note_forwarded_enclosing_param(
                call,
                async_doctor_core::DecoratorRef::Function(fid),
            );
            let kind = if self.pending_with_enter {
                CallKind::WithEnter
            } else if self.consuming_generator && self.function(fid).is_generator {
                CallKind::GeneratorConsume
            } else {
                CallKind::Call
            };
            let is_coro = Some(self.function(fid).is_async);
            self.pending_is_bound_call = self.expr_is_bound_method_call(&call.func, fid);
            self.push_edge(
                caller,
                CalleeRef::Project(fid),
                call.range(),
                kind,
                false,
                is_coro,
            );
            self.clear_pending_call_args();
            return Some(None);
        }

        // Project class constructor (`Worker(...)`, `cls(...)`, inherited `__init__`,
        // `__new__`, `__post_init__`).
        let ctor_fids = self.resolve_project_constructors(&call.func);
        if !ctor_fids.is_empty() {
            let kind = if self.pending_with_enter {
                CallKind::WithEnter
            } else {
                CallKind::Call
            };
            let saved_literals = self.pending_literals.clone();
            let saved_kw_literals = self.pending_keyword_literals.clone();
            let saved_callables = self.pending_callables.clone();
            let saved_kw_callables = self.pending_keyword_callables.clone();
            for (idx, fid) in ctor_fids.into_iter().enumerate() {
                if idx > 0 {
                    self.pending_literals = saved_literals.clone();
                    self.pending_keyword_literals = saved_kw_literals.clone();
                    self.pending_callables = saved_callables.clone();
                    self.pending_keyword_callables = saved_kw_callables.clone();
                }
                let is_coro = Some(self.function(fid).is_async);
                self.pending_is_bound_call = true;
                self.push_edge(
                    caller,
                    CalleeRef::Project(fid),
                    call.range(),
                    kind,
                    false,
                    is_coro,
                );
            }
            self.clear_pending_call_args();
            return Some(None);
        }

        let func_ext_qual = self.external_qualname(&call.func);
        self.note_socket_mode_call(call, func_ext_qual.as_deref());
        self.maybe_record_gather_shared_session(caller, call, func_ext_qual.as_deref());

        // In-memory buffer args to open/Image.open: not blocking file IO — skip
        // the edge (and therefore follow into PIL internals that would open paths).
        if let Some(q) = func_ext_qual.as_deref()
            && is_path_or_file_open_qual(q)
            && self.first_arg_is_in_memory_buffer(call)
        {
            self.clear_pending_call_args();
            self.pending_is_awaited = false;
            return Some(None);
        }

        // zipfile/tarfile on BytesIO/StringIO (ctor or subsequent receiver ops).
        if self.call_is_in_memory_archive(call, func_ext_qual.as_deref()) {
            self.clear_pending_call_args();
            self.pending_is_awaited = false;
            return Some(None);
        }

        // Non-blocking lock/queue variants (`acquire(False)`, `join(timeout=0)`,
        // `put` on unbounded Queue, …) — skip the edge entirely.
        if let Some(q) = func_ext_qual.as_deref()
            && self.is_proven_nonblocking_lock_call(q, call)
        {
            self.clear_pending_call_args();
            self.pending_is_awaited = false;
            return Some(None);
        }

        if let Some(kind) = func_ext_qual
            .as_deref()
            .and_then(Self::offload_kind_from_qualname)
        {
            self.pending_externals =
                self.resolve_positional_externals(call.arguments.args.as_ref());
            self.pending_constructed_receivers =
                self.resolve_positional_constructed_receivers(call.arguments.args.as_ref());
            let callable_idx = kind.callable_arg_index();
            let effective =
                self.effective_offload_callable(call.arguments.args.as_ref(), callable_idx);
            // Passing a callable param into a real offload → this function is a wrapper.
            if let Some(Expr::Name(n)) = effective {
                if self.current_param_names.iter().any(|p| p == n.id.as_str()) {
                    self.mark_offload_wrapper(caller);
                } else if let Some(owner) = self.enclosing_param_scope(n.id.as_str()) {
                    // `D(fn) -> wrapper` where the returned nested def offloads
                    // `fn` — the decorator factory is an offload wrapper.
                    self.mark_offload_wrapper(owner);
                }
            }
            // Align `callable_idx` slots with the unwrapped target (e.g. after `ctx.run`)
            // and handle coroutine-returning lambdas (`lambda: async_fn()`).
            let target_coro = if let Some(target) = effective {
                if let Some(lam) = self.resolve_lambda_expr(target) {
                    let (fid, is_coro) = self.resolve_lambda_callable_and_coro(lam);
                    if callable_idx < self.pending_callables.len() {
                        self.pending_callables[callable_idx] = fid;
                    }
                    if callable_idx < self.pending_externals.len() {
                        self.pending_externals[callable_idx] = None;
                    }
                    if callable_idx < self.pending_constructed_receivers.len() {
                        self.pending_constructed_receivers[callable_idx] = None;
                    }
                    Some(is_coro)
                } else {
                    let proj_fid = self.resolve_callable_project_fid(target);
                    if callable_idx < self.pending_callables.len() {
                        self.pending_callables[callable_idx] = proj_fid;
                    }
                    if callable_idx < self.pending_externals.len() {
                        self.pending_externals[callable_idx] = if proj_fid.is_some() {
                            None
                        } else {
                            self.external_qualname(target)
                        };
                    }
                    if callable_idx < self.pending_constructed_receivers.len() {
                        self.pending_constructed_receivers[callable_idx] =
                            self.constructed_root_receiver(target);
                    }
                    self.resolve_callee_is_coroutine(target)
                }
            } else {
                None
            };
            self.push_edge(
                caller,
                CalleeRef::Offload(kind),
                call.range(),
                CallKind::Call,
                true,
                target_coro,
            );
            self.maybe_record_eager_offload_call(caller, call, kind, call.range());
            self.maybe_record_offload_stateful_session(caller, effective, kind, call.range());
            self.clear_pending_call_args();
            return Some(Some(kind));
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
                None,
            );
            self.clear_pending_call_args();
            return Some(None);
        }

        if let Some(callee) = self.resolve_non_project_callee(&call.func, func_ext_qual) {
            if let CalleeRef::External { qualname } = &callee {
                self.note_forwarded_enclosing_param(
                    call,
                    async_doctor_core::DecoratorRef::External(qualname.clone()),
                );
            }
            if let CalleeRef::External { qualname } = &callee
                && qualname.starts_with("openai.")
            {
                self.pending_constructed_receiver = match call.func.as_ref() {
                    Expr::Attribute(_) => self.constructed_root_receiver(call.func.as_ref()),
                    _ => None,
                };
            }
            let kind = if self.pending_with_enter && !self.pending_is_awaited {
                CallKind::WithEnter
            } else if self.consuming_generator && matches!(&callee, CalleeRef::External { .. }) {
                // External consume-only sinks (`listen()`).
                CallKind::GeneratorConsume
            } else {
                CallKind::Call
            };
            let is_coro = self.resolve_callee_is_coroutine(&call.func);
            if matches!(&callee, CalleeRef::External { .. }) {
                self.pending_is_bound_call = self.expr_is_external_bound_call(&call.func);
            }
            self.push_edge(caller, callee, call.range(), kind, false, is_coro);
        }
        self.clear_pending_call_args();
        Some(None)
    }
}
