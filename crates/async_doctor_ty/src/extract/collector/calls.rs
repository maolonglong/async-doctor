//! Call / await / property / lambda / task edge recording.

use async_doctor_core::{CallKind, CalleeRef, EntryLabel, FunctionFact, FunctionId};
use ruff_db::files::FilePath;
use ruff_python_ast::visitor::Visitor;
use ruff_python_ast::{self as ast, Expr};
use ruff_text_size::Ranged;
use ty_python_semantic::{ImportAliasResolution, definitions_for_attribute, definitions_for_name};

use super::CallCollector;
use crate::extract::constants::{is_generator_consumer, literal_arg};
use crate::extract::ctx::DefKey;
use crate::extract::helpers::{
    is_asyncio_redis_type, is_callable_ref, is_getattr_call, is_in_memory_buffer_type,
    is_partial_call, is_path_or_file_open_qual, is_task_spawn_callee, normalize_asyncio_redis_type,
    qualname_from_definition, route_handler_arg, twin_relevant_client_root,
};

impl<'a, 'db> CallCollector<'a, 'db> {
    pub(crate) fn walk_call_children(&mut self, call: &'a ast::ExprCall) {
        let offload = self.is_offload_callee(&call.func);
        let consumer = is_generator_consumer(&call.func);
        if offload.is_some() {
            for (i, arg) in call.arguments.args.iter().enumerate() {
                if i == 0 && is_callable_ref(arg) {
                    continue;
                }
                // Non-callable (or non-first) args of an offload are evaluated on
                // the event loop before the offload runs — mark eager.
                let prev_eager = self.in_eager_offload_arg;
                self.in_eager_offload_arg = true;
                if consumer {
                    self.consuming_generator = true;
                }
                self.visit_expr(arg);
                if consumer {
                    self.consuming_generator = false;
                }
                self.in_eager_offload_arg = prev_eager;
            }
            for kw in &call.arguments.keywords {
                let prev_eager = self.in_eager_offload_arg;
                if !is_callable_ref(&kw.value) {
                    self.in_eager_offload_arg = true;
                }
                self.visit_expr(&kw.value);
                self.in_eager_offload_arg = prev_eager;
            }
        } else if consumer {
            self.in_callee_position = true;
            self.visit_expr(&call.func);
            self.in_callee_position = false;
            self.consuming_generator = true;
            for arg in &call.arguments.args {
                self.visit_expr(arg);
            }
            for kw in &call.arguments.keywords {
                self.visit_expr(&kw.value);
            }
            self.consuming_generator = false;
        } else {
            self.in_callee_position = true;
            self.visit_expr(&call.func);
            self.in_callee_position = false;
            for arg in &call.arguments.args {
                self.visit_expr(arg);
            }
            for kw in &call.arguments.keywords {
                self.visit_expr(&kw.value);
            }
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

    pub(crate) fn handle_call(&mut self, call: &'a ast::ExprCall, is_awaited: bool) {
        self.pending_is_awaited = is_awaited;
        // Module-level and function-level: task spawns + imperative routes.
        self.note_task_spawn(call);
        self.note_imperative_route(call);

        let Some(caller) = self.current_fn else {
            return;
        };
        if self.dead_depth > 0 || self.in_lambda {
            return;
        }

        // Lambda assigned to a local then called: `f = lambda: ...; f()`.
        if let Expr::Name(name) = call.func.as_ref()
            && let Some(lambda) = self.local_lambdas.get(name.id.as_str()).copied()
        {
            self.analyze_lambda_body(lambda);
        }

        self.pending_literals = call.arguments.args.iter().map(literal_arg).collect();
        self.pending_callables = self.resolve_positional_callables(call.arguments.args.as_ref());
        self.pending_externals = self.resolve_positional_externals(call.arguments.args.as_ref());
        self.pending_constructed_receivers =
            self.resolve_positional_constructed_receivers(call.arguments.args.as_ref());
        self.pending_constructed_receiver = match call.func.as_ref() {
            Expr::Attribute(_) => self.constructed_root_receiver(call.func.as_ref()),
            _ => None,
        };
        self.pending_keyword_callables = self.resolve_keyword_callables(call);

        // In-memory buffer args to open/Image.open: not blocking file IO — skip
        // the edge (and therefore follow into PIL internals that would open paths).
        if let Some(q) = self.external_qualname(&call.func)
            && is_path_or_file_open_qual(&q)
            && self.first_arg_is_in_memory_buffer(call)
        {
            self.clear_pending_call_args();
            self.pending_is_awaited = false;
            return;
        }

        if let Some(kind) = self.is_offload_callee(&call.func) {
            // Passing a callable param into a real offload → this function is a wrapper.
            let callable_idx = kind.callable_arg_index();
            if let Some(arg) = call.arguments.args.get(callable_idx)
                && let Expr::Name(n) = arg
                && self.current_param_names.iter().any(|p| p == n.id.as_str())
            {
                self.facts.functions[caller.index()].is_offload_wrapper = true;
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
                None,
            );
            self.clear_pending_call_args();
            return;
        }

        if let Some(callee) = self.resolve_callee(&call.func) {
            let kind = if self.pending_with_enter {
                CallKind::WithEnter
            } else if self.consuming_generator
                && (matches!(
                    &callee,
                    CalleeRef::Project(id) if self.facts.function(*id).is_generator
                ) || matches!(&callee, CalleeRef::External { .. }))
            {
                // Project generators and external consume-only sinks (`listen()`).
                CallKind::GeneratorConsume
            } else {
                CallKind::Call
            };
            let is_coro = self.resolve_callee_is_coroutine(&call.func);
            self.push_edge(caller, callee, call.range(), kind, false, is_coro);
        }
        self.clear_pending_call_args();
    }

    fn clear_pending_call_args(&mut self) {
        self.pending_literals.clear();
        self.pending_callables.clear();
        self.pending_externals.clear();
        self.pending_constructed_receivers.clear();
        self.pending_constructed_receiver = None;
        self.pending_keyword_callables.clear();
    }

    pub(crate) fn first_arg_is_in_memory_buffer(&mut self, call: &ast::ExprCall) -> bool {
        let Some(arg) = call.arguments.args.first() else {
            // Also accept fp=/file= kwargs used by PIL.
            let kw = call.arguments.keywords.iter().find(|k| {
                k.arg
                    .as_ref()
                    .is_some_and(|a| matches!(a.as_str(), "fp" | "file" | "filename"))
            });
            return kw.is_some_and(|k| self.expr_is_in_memory_buffer(&k.value));
        };
        self.expr_is_in_memory_buffer(arg)
    }

    pub(crate) fn expr_is_in_memory_buffer(&mut self, expr: &Expr) -> bool {
        if let Expr::Call(c) = expr {
            // BytesIO(...) / StringIO(...) constructor call.
            if let Some(q) = self.external_qualname(&c.func)
                && is_in_memory_buffer_type(&q)
            {
                return true;
            }
            if let Expr::Attribute(a) = c.func.as_ref()
                && matches!(a.attr.as_str(), "BytesIO" | "StringIO")
            {
                return true;
            }
            if let Expr::Name(n) = c.func.as_ref()
                && matches!(n.id.as_str(), "BytesIO" | "StringIO")
            {
                return true;
            }
        }
        if let Some(ty) = self.type_instance_qualname(expr)
            && is_in_memory_buffer_type(&ty)
        {
            return true;
        }
        false
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
                Expr::Name(_) | Expr::Attribute(_) => self.resolve_project_function(&kw.value),
                Expr::Call(c) if is_partial_call(c) => c
                    .arguments
                    .args
                    .first()
                    .and_then(|a| self.resolve_project_function(a)),
                Expr::Lambda(lam) => Some(self.ensure_lambda_fact(lam)),
                _ => None,
            };
            if let Some(fid) = fid {
                out.push((name.as_str().to_string(), fid));
            }
        }
        out
    }

    pub(crate) fn note_local_assign(&mut self, assign: &'a ast::StmtAssign) {
        if assign.targets.len() != 1 {
            return;
        }
        let Expr::Name(n) = &assign.targets[0] else {
            return;
        };
        self.note_local_binding(n.id.as_str(), &assign.value);
    }

    pub(crate) fn note_local_binding(&mut self, name: &str, value: &'a Expr) {
        match value {
            Expr::Lambda(lam) => {
                self.local_lambdas.insert(name.to_string(), lam);
                self.idle_loop_locals.remove(name);
            }
            Expr::Generator(gexpr) => {
                self.local_genexprs.insert(name.to_string(), gexpr);
                self.idle_loop_locals.remove(name);
            }
            other => {
                self.local_lambdas.remove(name);
                self.local_genexprs.remove(name);
                if super::loop_guards::value_is_idle_loop_binding(other, &self.idle_loop_helpers) {
                    self.idle_loop_locals.insert(name.to_string());
                } else {
                    self.idle_loop_locals.remove(name);
                }
            }
        }
    }

    pub(crate) fn analyze_lambda_body(&mut self, lambda: &'a ast::ExprLambda) {
        let prev_in = self.in_lambda;
        let depths = self.take_enclosing_depths();
        self.in_lambda = false;
        self.visit_expr(&lambda.body);
        self.in_lambda = prev_in;
        self.restore_enclosing_depths(depths);
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
        let is_prop = self
            .property_defs
            .iter()
            .any(|k| self.def_get(k) == Some(fid));
        if !is_prop {
            return;
        }
        self.clear_pending_call_args();
        self.push_edge(
            caller,
            CalleeRef::Project(fid),
            attr.range(),
            CallKind::PropertyGet,
            false,
            Some(self.facts.function(fid).is_async),
        );
    }

    pub(crate) fn note_imperative_route(&mut self, call: &ast::ExprCall) {
        let Some(handler) = route_handler_arg(call) else {
            return;
        };
        let Some(fid) = self.resolve_project_function(handler) else {
            return;
        };
        let f = &mut self.facts.functions[fid.index()];
        if f.label == EntryLabel::Other {
            f.label = EntryLabel::Route;
        }
        if !f.is_async {
            f.is_sync_route = true;
        }
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
                Expr::Name(_) | Expr::Attribute(_) => self.resolve_project_function(arg),
                Expr::Call(c) if is_partial_call(c) => c
                    .arguments
                    .args
                    .first()
                    .and_then(|a| self.resolve_project_function(a)),
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
            if self.resolve_project_function(target).is_some() {
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

    pub(crate) fn ensure_lambda_fact(&mut self, lam: &'a ast::ExprLambda) -> FunctionId {
        let key = DefKey {
            file: self.ctx.rel.clone(),
            name_start: u32::from(lam.range.start()),
        };
        if let Some(id) = self.def_get(&key) {
            return id;
        }
        let id = FunctionId(self.facts.functions.len() as u32);
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
        });
        // Analyze lambda body as edges from the synthetic function.
        // Judge body edges relative to the lambda itself — enclosing try/if/loop
        // tags on the offload site must not make lambda sinks look conditional.
        let prev = self.current_fn;
        let prev_params = std::mem::take(&mut self.current_param_names);
        let prev_in = self.in_lambda;
        let depths = self.take_enclosing_depths();
        self.current_fn = Some(id);
        self.current_param_names = self.facts.function(id).param_names.clone();
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
    fn take_enclosing_depths(&mut self) -> (u32, u32, u32, u32, u32) {
        let depths = (
            self.dead_depth,
            self.loop_depth,
            self.branch_depth,
            self.cache_miss_depth,
            self.no_running_loop_depth,
        );
        self.dead_depth = 0;
        self.loop_depth = 0;
        self.branch_depth = 0;
        self.cache_miss_depth = 0;
        self.no_running_loop_depth = 0;
        depths
    }

    fn restore_enclosing_depths(&mut self, depths: (u32, u32, u32, u32, u32)) {
        self.dead_depth = depths.0;
        self.loop_depth = depths.1;
        self.branch_depth = depths.2;
        self.cache_miss_depth = depths.3;
        self.no_running_loop_depth = depths.4;
    }
}
