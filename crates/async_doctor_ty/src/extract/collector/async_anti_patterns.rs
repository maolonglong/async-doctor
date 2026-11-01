//! Detection of async anti-patterns during pass-2 AST collection:
//! - `sync-with-async-cm`: sync `with` entering an async-only context manager
//! - `dangling-task`: bare `Stmt::Expr` calling `asyncio.create_task` / `ensure_future`

use async_doctor_core::{CallKind, CalleeRef};
use async_doctor_rules::{RULE_DANGLING_TASK, RULE_SYNC_WITH_ASYNC_CM};
use ruff_db::files::FilePath;
use ruff_db::parsed::parsed_module;
use ruff_python_ast::{self as ast, Expr, Stmt};
use ruff_text_size::Ranged;
use ty_python_core::definition::{Definition, DefinitionKind};
use ty_python_semantic::types::TypeDefinition;
use ty_python_semantic::{
    HasType, ImportAliasResolution, definitions_for_attribute, definitions_for_name,
};

use super::CallCollector;
use super::loop_guards::{expr_is_get_event_loop, expr_is_get_running_loop};

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
            | "anyio.fail_after"
            | "anyio.move_on_after"
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

impl<'a, 'db> CallCollector<'a, 'db> {
    /// True when `expr` inside a sync `with` item is proven to be an async-only
    /// context manager (raises `TypeError` at runtime when entered via `with`).
    pub(crate) fn is_sync_with_async_cm(&mut self, expr: &Expr) -> bool {
        if let Expr::Call(call) = expr {
            // Calling an `async def` (coroutine or async generator) in a sync `with`
            // always produces a coroutine / async_generator without `__enter__`.
            if self.resolve_callee_is_coroutine(&call.func) == Some(true) {
                return true;
            }
            if self.expr_resolves_to_async_only_project_class(&call.func) {
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
                && self.class_def_is_async_only_cm(def)
            {
                return true;
            }
        }

        false
    }

    /// Record a `sync-with-async-cm` edge for a sync `with` item.
    pub(crate) fn push_sync_with_async_cm_edge(
        &mut self,
        caller: async_doctor_core::FunctionId,
        expr: &Expr,
    ) {
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

    fn expr_resolves_to_async_only_project_class(&self, expr: &Expr) -> bool {
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
            if let Some(def) = r.definition()
                && self.class_def_is_async_only_cm(def)
            {
                return true;
            }
        }
        false
    }

    fn class_def_is_async_only_cm(&self, def: Definition<'db>) -> bool {
        let file = def.file(self.model.db());
        let FilePath::System(_) = file.path(self.model.db()) else {
            return false;
        };
        let DefinitionKind::Class(cdef) = def.kind(self.model.db()) else {
            return false;
        };
        let parsed =
            parsed_module(self.model.db(), def.python_file(self.model.db())).load(self.model.db());
        let class_node = cdef.node(&parsed);
        // Only flag classes without base classes so an inherited `__enter__` is never missed.
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

    /// Record a `dangling-task` edge when a bare `Stmt::Expr` calls
    /// `asyncio.create_task` / `loop.create_task` / `asyncio.ensure_future`
    /// and discards the returned `Task`.
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
