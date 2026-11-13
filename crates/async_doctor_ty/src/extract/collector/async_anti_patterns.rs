//! Detection of async protocol & statement anti-patterns during pass-2 AST collection:
//! - `sync-with-async-cm`: sync `with` entering an async-only context manager
//! - `sync-for-async-iterable`: sync `for` / sync comprehension over async-only iterable
//! - `unawaited-coroutine`: bare `Stmt::Expr` calling a proven coroutine function without `await`
//! - `dangling-task`: bare `Stmt::Expr` calling `asyncio.create_task` / `ensure_future`

use async_doctor_core::{CallKind, CalleeRef, FunctionId};
use async_doctor_rules::{
    RULE_DANGLING_TASK, RULE_SYNC_FOR_ASYNC_ITERABLE, RULE_SYNC_WITH_ASYNC_CM,
    RULE_UNAWAITED_COROUTINE,
};
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

use super::CallCollector;
use super::loop_guards::{expr_is_get_event_loop, expr_is_get_running_loop};
use crate::extract::constants::body_has_yield;
use crate::extract::functions::decorator_is_route;
use crate::extract::helpers::module_of_definition;

pub(super) fn is_known_async_only_cm_qual(qualname: &str) -> bool {
    let q = qualname.strip_suffix(".__enter__").unwrap_or(qualname);
    matches!(
        q,
        "asyncio.Lock"
            | "asyncio.locks.Lock"
            | "asyncio.Semaphore"
            | "asyncio.locks.Semaphore"
            | "asyncio.BoundedSemaphore"
            | "asyncio.locks.BoundedSemaphore"
            | "asyncio.Condition"
            | "asyncio.locks.Condition"
            | "asyncio.Barrier"
            | "asyncio.locks.Barrier"
            | "asyncio.TaskGroup"
            | "asyncio.taskgroups.TaskGroup"
            | "asyncio.timeout"
            | "asyncio.timeout_at"
            | "asyncio.timeouts.timeout"
            | "asyncio.timeouts.timeout_at"
            | "asyncio.timeouts.Timeout"
            | "anyio.create_task_group"
            // anyio.fail_after / move_on_after are sync context managers in
            // anyio 4.x (`__enter__`, no `__aenter__`); `with` is correct.
            | "anyio.Lock"
            | "anyio.Semaphore"
            | "anyio.CapacityLimiter"
            | "anyio.Condition"
            | "httpx.AsyncClient"
            | "httpx._client.AsyncClient"
            | "aiohttp.ClientSession"
            | "aiohttp.client.ClientSession"
            | "sqlalchemy.ext.asyncio.AsyncSession"
            | "sqlalchemy.ext.asyncio.session.AsyncSession"
            | "sqlalchemy.ext.asyncio.AsyncEngine.begin"
            | "sqlalchemy.ext.asyncio.AsyncEngine.connect"
            | "sqlalchemy.ext.asyncio.engine.AsyncEngine.begin"
            | "sqlalchemy.ext.asyncio.engine.AsyncEngine.connect"
            | "sqlalchemy.ext.asyncio.AsyncConnection"
            | "sqlalchemy.ext.asyncio.engine.AsyncConnection"
            | "aioboto3.Session.client"
            | "aioboto3.Session.resource"
            | "aioboto3.session.Session.client"
            | "aioboto3.session.Session.resource"
            | "aiobotocore.session.AioSession.create_client"
            | "redis.asyncio.Redis.pipeline"
            | "redis.asyncio.client.Redis.pipeline"
            | "redis.asyncio.client.Pipeline"
            | "redis.asyncio.Redis.lock"
            | "redis.asyncio.client.Redis.lock"
            | "psycopg.AsyncConnection"
            | "psycopg.AsyncConnection.connect"
            | "psycopg.AsyncCursor"
            | "psycopg_pool.AsyncConnectionPool.connection"
            | "asyncpg.Pool.acquire"
            | "asyncpg.pool.Pool.acquire"
            | "asyncpg.Connection.transaction"
    )
}

fn is_known_async_only_iterable_call_qual(qualname: &str) -> bool {
    let q = qualname.strip_suffix(".__iter__").unwrap_or(qualname);
    matches!(
        q,
        "httpx.Response.aiter_bytes"
            | "httpx.Response.aiter_text"
            | "httpx.Response.aiter_lines"
            | "httpx.Response.aiter_raw"
            | "httpx._models.Response.aiter_bytes"
            | "httpx._models.Response.aiter_text"
            | "httpx._models.Response.aiter_lines"
            | "httpx._models.Response.aiter_raw"
            | "redis.asyncio.Redis.scan_iter"
            | "redis.asyncio.client.Redis.scan_iter"
            | "redis.asyncio.Redis.sscan_iter"
            | "redis.asyncio.client.Redis.sscan_iter"
            | "redis.asyncio.Redis.hscan_iter"
            | "redis.asyncio.client.Redis.hscan_iter"
            | "redis.asyncio.Redis.zscan_iter"
            | "redis.asyncio.client.Redis.zscan_iter"
            | "redis.asyncio.PubSub.listen"
            | "redis.asyncio.client.PubSub.listen"
            | "aiohttp.StreamReader.iter_chunked"
            | "aiohttp.StreamReader.iter_any"
            | "aiohttp.StreamReader.iter_chunks"
            | "aiohttp.streams.StreamReader.iter_chunked"
            | "aiohttp.streams.StreamReader.iter_any"
            | "aiohttp.streams.StreamReader.iter_chunks"
            | "asyncpg.Connection.cursor"
            | "asyncpg.connection.Connection.cursor"
            | "sqlalchemy.ext.asyncio.AsyncSession.stream"
            | "sqlalchemy.ext.asyncio.session.AsyncSession.stream"
            | "sqlalchemy.ext.asyncio.AsyncSession.stream_scalars"
            | "sqlalchemy.ext.asyncio.session.AsyncSession.stream_scalars"
            | "sqlalchemy.ext.asyncio.AsyncConnection.stream"
            | "sqlalchemy.ext.asyncio.engine.AsyncConnection.stream"
            | "sqlalchemy.ext.asyncio.AsyncConnection.stream_scalars"
            | "sqlalchemy.ext.asyncio.engine.AsyncConnection.stream_scalars"
    )
}

fn is_known_async_only_iterable_type_qual(qualname: &str) -> bool {
    matches!(
        qualname,
        "builtins.coroutine"
            | "types.CoroutineType"
            | "builtins.async_generator"
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
            | "sqlalchemy.ext.asyncio.result.AsyncResult"
            | "sqlalchemy.ext.asyncio.AsyncResult"
            | "sqlalchemy.ext.asyncio.result.AsyncScalarResult"
            | "sqlalchemy.ext.asyncio.AsyncScalarResult"
            | "sqlalchemy.ext.asyncio.result.AsyncMappingResult"
            | "aiohttp.streams.StreamReader"
            | "aiohttp.StreamReader"
            | "aiokafka.consumer.consumer.AIOKafkaConsumer"
            | "aiokafka.AIOKafkaConsumer"
            | "asyncpg.cursor.CursorFactory"
            | "asyncpg.cursor.Cursor"
    )
}

/// Decorators that preserve an `async def`'s coroutine return semantics.
/// Excludes `@asynccontextmanager`, `@async_to_sync`, task queues, and custom
/// wrappers that may execute synchronously or alter the return type.
pub(super) fn decorator_preserves_coroutine(expr: &Expr) -> bool {
    if decorator_is_route(expr) {
        return true;
    }
    match expr {
        Expr::Name(n) => matches!(
            n.id.as_str(),
            "staticmethod" | "classmethod" | "overload" | "abstractmethod" | "override"
        ),
        Expr::Attribute(attr) => matches!(
            attr.attr.as_str(),
            "overload" | "abstractmethod" | "override"
        ),
        _ => false,
    }
}

fn return_annotation_mentions_async_iter(src: &str, func: &ast::StmtFunctionDef) -> bool {
    let Some(ret) = &func.returns else {
        return false;
    };
    let start = usize::from(ret.range().start());
    let end = usize::from(ret.range().end()).min(src.len());
    if start >= end {
        return false;
    }
    let snippet = &src[start..end];
    snippet.contains("AsyncIterator")
        || snippet.contains("AsyncGenerator")
        || snippet.contains("AsyncIterable")
}

impl<'a, 'db> CallCollector<'a, 'db> {
    /// True when `expr` inside a sync `with` item is proven to be an async-only
    /// context manager (raises `TypeError` at runtime when entered via `with`).
    pub(crate) fn is_sync_with_async_cm(&mut self, expr: &Expr) -> bool {
        if let Expr::Call(call) = expr {
            if self.callee_is_unwrapped_async_def(&call.func) {
                return true;
            }
            if self.expr_resolves_to_project_class_matching(&call.func, |c| {
                Self::class_node_is_async_only_cm(c)
            }) {
                return true;
            }
            if let Some(q) = self.external_qualname(&call.func)
                && is_known_async_only_cm_qual(&q)
            {
                return true;
            }
        }

        if let Some(ty_qual) = self.type_instance_qualname(expr)
            && is_known_async_only_cm_qual(&ty_qual)
        {
            return true;
        }

        if let Some(ty) = expr.inferred_type(self.model) {
            let env = self.model.program_environment();
            if let Some(TypeDefinition::StaticClass(def) | TypeDefinition::DynamicClass(def)) =
                ty.definition(self.model.db(), &env)
                && self.class_def_matches(def, Self::class_node_is_async_only_cm)
            {
                return true;
            }
        }

        false
    }

    pub(crate) fn push_sync_with_async_cm_edge(&mut self, caller: FunctionId, expr: &Expr) {
        self.clear_pending_call_args();
        self.push_edge(
            caller,
            CalleeRef::External {
                qualname: RULE_SYNC_WITH_ASYNC_CM.to_string(),
            },
            expr.range(),
            CallKind::WithEnter,
            false,
            Some(false),
        );
    }

    /// True when `expr` in a sync `for` loop or sync comprehension is proven to
    /// be an async-only iterable / coroutine (`TypeError` at runtime).
    pub(crate) fn is_sync_for_async_iterable(&mut self, expr: &Expr) -> bool {
        if let Expr::Call(call) = expr {
            if self.callee_is_unwrapped_async_def(&call.func) {
                return true;
            }
            if self.expr_resolves_to_project_class_matching(&call.func, |c| {
                Self::class_node_is_async_only_iterable(c)
            }) {
                return true;
            }
            if let Some(q) = self.external_qualname(&call.func)
                && is_known_async_only_iterable_call_qual(&q)
            {
                return true;
            }
        }

        let Some(ty) = expr.inferred_type(self.model) else {
            return false;
        };
        let env = self.model.program_environment();
        self.type_is_async_only_iterable(ty, &env)
    }

    pub(crate) fn push_sync_for_async_iterable_edge(
        &mut self,
        caller: FunctionId,
        expr: &Expr,
        kind: CallKind,
    ) {
        self.clear_pending_call_args();
        self.push_edge(
            caller,
            CalleeRef::External {
                qualname: RULE_SYNC_FOR_ASYNC_ITERABLE.to_string(),
            },
            expr.range(),
            kind,
            false,
            Some(false),
        );
    }

    fn type_is_async_only_iterable(
        &self,
        ty: Type<'db>,
        env: &ty_python_semantic::ProgramEnvironment<'db>,
    ) -> bool {
        if matches!(
            ty,
            Type::ClassLiteral(_) | Type::SubclassOf(_) | Type::GenericAlias(_)
        ) {
            return false;
        }
        if let Some(union) = ty.as_union() {
            let mut saw_arm = false;
            for elem in union.elements(self.model.db()).iter() {
                if matches!(elem, Type::Never) {
                    continue;
                }
                saw_arm = true;
                if !self.type_is_async_only_iterable(*elem, env) {
                    return false;
                }
            }
            return saw_arm;
        }
        if let Some(TypeDefinition::StaticClass(def) | TypeDefinition::DynamicClass(def)) =
            ty.definition(self.model.db(), env)
        {
            if self.class_def_matches(def, Self::class_node_is_async_only_iterable) {
                return true;
            }
            if let Some(name) = def.name(self.model.db())
                && let Some(module) = module_of_definition(self.model, def)
            {
                let qual = format!("{module}.{name}");
                if is_known_async_only_iterable_type_qual(&qual) {
                    return true;
                }
            }
        }
        false
    }

    /// Record an `unawaited-coroutine` edge when a bare `Stmt::Expr` calls a
    /// proven coroutine-returning function and discards the coroutine.
    pub(crate) fn maybe_record_unawaited_coroutine(&mut self, expr_stmt: &ast::StmtExpr) {
        let Some(caller) = self.current_fn else {
            return;
        };
        if self.dead_depth > 0
            || self.in_lambda
            || self.off_loop_thread_depth > 0
            || self.function(caller).is_installed_third_party()
        {
            return;
        }
        let Expr::Call(call) = expr_stmt.value.as_ref() else {
            return;
        };
        if !self.call_returns_proven_coroutine(call) {
            return;
        }
        self.clear_pending_call_args();
        self.push_edge(
            caller,
            CalleeRef::External {
                qualname: RULE_UNAWAITED_COROUTINE.to_string(),
            },
            call.range(),
            CallKind::Call,
            false,
            Some(false),
        );
    }

    /// True when `func` resolves exclusively to `async def` functions with no
    /// return-altering decorators (covers both coroutines and async generators).
    pub(super) fn callee_is_unwrapped_async_def(&self, func: &Expr) -> bool {
        if !self.resolve_project_constructors(func).is_empty() {
            return false;
        }
        let Some(defs) = self.resolve_callee_function_defs(func) else {
            return false;
        };
        !defs.is_empty()
            && defs.iter().all(|def| {
                let DefinitionKind::Function(fdef) = def.kind(self.model.db()) else {
                    return false;
                };
                let parsed = parsed_module(self.model.db(), def.python_file(self.model.db()))
                    .load(self.model.db());
                let node = fdef.node(&parsed);
                node.is_async
                    && node
                        .decorator_list
                        .iter()
                        .all(|d| decorator_preserves_coroutine(&d.expression))
            })
    }

    /// True when `call` is proven to return a coroutine object (not an async
    /// generator, not `@asynccontextmanager`, not `@async_to_sync`).
    fn call_returns_proven_coroutine(&self, call: &ast::ExprCall) -> bool {
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

    pub(super) fn resolve_callee_function_defs(&self, func: &Expr) -> Option<Vec<Definition<'db>>> {
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

    pub(super) fn expr_resolves_to_project_class_matching(
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

    pub(super) fn class_def_matches(
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

    fn class_node_is_async_only_cm(class_node: &ast::StmtClassDef) -> bool {
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

    fn class_node_is_async_only_iterable(class_node: &ast::StmtClassDef) -> bool {
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
