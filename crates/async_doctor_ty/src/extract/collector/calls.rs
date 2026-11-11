//! Call / await / property / lambda / task edge recording.

use async_doctor_core::{CallKind, CalleeRef, FunctionId, OffloadKind};
use ruff_db::files::FilePath;
use ruff_python_ast::visitor::Visitor;
use ruff_python_ast::{self as ast, Expr};
use ruff_text_size::Ranged;
use ty_python_semantic::{ImportAliasResolution, definitions_for_attribute, definitions_for_name};

use super::CallCollector;
use crate::extract::constants::{is_generator_consumer, keyword_true_names, literal_arg};
use crate::extract::helpers::{
    is_asyncio_redis_type, is_callable_ref, is_getattr_call, is_partial_call,
    is_path_or_file_open_qual, is_task_spawn_callee, normalize_asyncio_redis_type,
    qualname_from_definition, route_handler_arg, twin_relevant_client_root,
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
            for (i, arg) in call.arguments.args.iter().enumerate() {
                if i == callable_idx {
                    if let Expr::Call(inner) = arg
                        && is_partial_call(inner)
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
                    && is_partial_call(inner)
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

    /// `functools.partial(fn, a, k=b)` passed to an offload defers `fn` (arg 0)
    /// while evaluating `a` and `b` eagerly on the event loop.
    fn visit_partial_eager_args(&mut self, inner: &'a ast::ExprCall, consumer: bool) {
        for eager_arg in inner.arguments.args.iter().skip(1) {
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

        self.pending_literals = call.arguments.args.iter().map(literal_arg).collect();
        self.pending_keyword_true = keyword_true_names(&call.arguments.keywords);
        self.pending_callables = self.resolve_positional_callables(call.arguments.args.as_ref());
        self.pending_externals.clear();
        self.pending_constructed_receivers.clear();
        self.pending_constructed_receiver = None;
        self.pending_keyword_callables = self.resolve_keyword_callables(call);

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
            let saved_kw_true = self.pending_keyword_true.clone();
            let saved_callables = self.pending_callables.clone();
            let saved_kw_callables = self.pending_keyword_callables.clone();
            for (idx, fid) in ctor_fids.into_iter().enumerate() {
                if idx > 0 {
                    self.pending_literals = saved_literals.clone();
                    self.pending_keyword_true = saved_kw_true.clone();
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
            // Passing a callable param into a real offload → this function is a wrapper.
            let callable_idx = kind.callable_arg_index();
            if let Some(arg) = call.arguments.args.get(callable_idx)
                && let Expr::Name(n) = arg
            {
                if self.current_param_names.iter().any(|p| p == n.id.as_str()) {
                    self.mark_offload_wrapper(caller);
                } else if let Some(owner) = self.enclosing_param_scope(n.id.as_str()) {
                    // `D(fn) -> wrapper` where the returned nested def offloads
                    // `fn` — the decorator factory is an offload wrapper.
                    self.mark_offload_wrapper(owner);
                }
            }
            // Prefer structural coroutine-ness of the offloaded target when known.
            let target_coro = call
                .arguments
                .args
                .get(callable_idx)
                .and_then(|arg| match arg {
                    Expr::Call(inner) if is_partial_call(inner) => inner.arguments.args.first(),
                    other => Some(other),
                })
                .and_then(|target| self.resolve_callee_is_coroutine(target));
            self.push_edge(
                caller,
                CalleeRef::Offload(kind),
                call.range(),
                CallKind::Call,
                true,
                target_coro,
            );
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
            let kind = if self.pending_with_enter {
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

    pub(crate) fn clear_pending_call_args(&mut self) {
        self.pending_literals.clear();
        self.pending_keyword_true.clear();
        self.pending_callables.clear();
        self.pending_externals.clear();
        self.pending_constructed_receivers.clear();
        self.pending_constructed_receiver = None;
        self.pending_keyword_callables.clear();
        self.pending_is_bound_call = false;
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
            let fid = match &kw.value {
                Expr::Name(_) | Expr::Attribute(_) => {
                    self.resolve_project_function(&kw.value).or_else(|| {
                        self.resolve_project_constructors(&kw.value)
                            .into_iter()
                            .next()
                    })
                }
                Expr::Call(c) if is_partial_call(c) => c.arguments.args.first().and_then(|a| {
                    self.resolve_project_function(a)
                        .or_else(|| self.resolve_project_constructors(a).into_iter().next())
                }),
                Expr::Lambda(lam) => Some(self.ensure_lambda_fact(lam)),
                _ => None,
            };
            if let Some(fid) = fid {
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
            let fid = match arg {
                Expr::Name(_) | Expr::Attribute(_) => self
                    .resolve_project_function(arg)
                    .or_else(|| self.resolve_project_constructors(arg).into_iter().next()),
                Expr::Call(c) if is_partial_call(c) => c.arguments.args.first().and_then(|a| {
                    self.resolve_project_function(a)
                        .or_else(|| self.resolve_project_constructors(a).into_iter().next())
                }),
                Expr::Lambda(lam) => Some(self.ensure_lambda_fact(lam)),
                _ => None,
            };
            out.push(fid);
        }
        out
    }

    /// External qualnames for positional args (offload of typed third-party methods).
    pub(crate) fn resolve_positional_externals(&mut self, args: &'a [Expr]) -> Vec<Option<String>> {
        let mut out = Vec::with_capacity(args.len());
        for arg in args {
            // Prefer project FunctionId when both resolve — externals are a fallback.
            if matches!(arg, Expr::Lambda(_)) {
                out.push(None);
                continue;
            }
            let target = match arg {
                Expr::Call(c) if is_partial_call(c) => c.arguments.args.first(),
                other => Some(other),
            };
            let Some(target) = target else {
                out.push(None);
                continue;
            };
            if self.resolve_project_function(target).is_some()
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
            let target = match arg {
                Expr::Call(c) if is_partial_call(c) => c.arguments.args.first(),
                Expr::Lambda(_) => {
                    out.push(None);
                    continue;
                }
                other => Some(other),
            };
            let Some(target) = target else {
                out.push(None);
                continue;
            };
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
