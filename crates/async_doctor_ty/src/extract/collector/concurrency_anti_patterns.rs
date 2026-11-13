//! Detection of async client/session lifecycle & concurrency anti-patterns:
//! - `ephemeral-async-client` (performance advice): per-route / per-iteration
//!   construction + teardown of `httpx.AsyncClient` / `aiohttp.ClientSession`.
//! - `shared-async-session` (correctness advice): sharing one stateful
//!   `AsyncSession` / `AsyncConnection` / `asyncpg.Connection` / `psycopg.AsyncConnection`
//!   across concurrent tasks in `asyncio.gather` or `TaskGroup`.

use std::collections::{BTreeMap, HashSet};

use async_doctor_core::{CallKind, CalleeRef, FunctionId};
use ruff_python_ast::{self as ast, Expr, Stmt};
use ruff_text_size::Ranged;
use ty_python_semantic::HasType;
use ty_python_semantic::types::{Type, TypeDefinition};

use super::CallCollector;
use crate::extract::helpers::module_of_definition;

fn is_stateful_async_db_handle_qual(qualname: &str) -> bool {
    matches!(
        qualname,
        "sqlalchemy.ext.asyncio.AsyncSession"
            | "sqlalchemy.ext.asyncio.session.AsyncSession"
            | "sqlalchemy.ext.asyncio.AsyncConnection"
            | "sqlalchemy.ext.asyncio.engine.AsyncConnection"
            | "sqlmodel.ext.asyncio.session.AsyncSession"
            | "asyncpg.Connection"
            | "asyncpg.connection.Connection"
            | "asyncpg.pool.PoolConnectionProxy"
            | "psycopg.AsyncConnection"
            | "psycopg.AsyncCursor"
            | "psycopg.cursor_async.AsyncCursor"
    )
}

fn is_stateful_session_factory_call_qual(qualname: &str) -> bool {
    matches!(
        qualname,
        "sqlalchemy.ext.asyncio.AsyncSession"
            | "sqlalchemy.ext.asyncio.session.AsyncSession"
            | "sqlalchemy.ext.asyncio.AsyncConnection"
            | "sqlalchemy.ext.asyncio.engine.AsyncConnection"
            | "sqlalchemy.ext.asyncio.AsyncEngine.connect"
            | "sqlalchemy.ext.asyncio.AsyncEngine.begin"
            | "sqlalchemy.ext.asyncio.engine.AsyncEngine.connect"
            | "sqlalchemy.ext.asyncio.engine.AsyncEngine.begin"
            | "sqlmodel.ext.asyncio.session.AsyncSession"
            | "asyncpg.connect"
            | "asyncpg.connection.connect"
            | "asyncpg.Pool.acquire"
            | "asyncpg.pool.Pool.acquire"
            | "psycopg.AsyncConnection.connect"
            | "psycopg.AsyncConnection.cursor"
            | "psycopg_pool.AsyncConnectionPool.connection"
    )
}

fn is_sessionmaker_type_qual(qualname: &str) -> bool {
    matches!(
        qualname,
        "sqlalchemy.ext.asyncio.async_sessionmaker"
            | "sqlalchemy.ext.asyncio.session.async_sessionmaker"
    )
}

fn is_task_group_cm_qual(qualname: &str) -> bool {
    matches!(
        qualname,
        "asyncio.TaskGroup" | "asyncio.taskgroups.TaskGroup" | "anyio.create_task_group"
    )
}

/// Collect `global` and `nonlocal` names declared directly inside `body`
/// (excluding nested `def` / `class` scopes).
pub(super) fn nonlocal_or_global_names(body: &[Stmt]) -> HashSet<String> {
    let mut out = HashSet::new();
    fn walk(stmts: &[Stmt], out: &mut HashSet<String>) {
        for stmt in stmts {
            match stmt {
                Stmt::Global(g) => {
                    for name in &g.names {
                        out.insert(name.as_str().to_string());
                    }
                }
                Stmt::Nonlocal(nl) => {
                    for name in &nl.names {
                        out.insert(name.as_str().to_string());
                    }
                }
                Stmt::If(s) => {
                    walk(&s.body, out);
                    for c in &s.elif_else_clauses {
                        walk(&c.body, out);
                    }
                }
                Stmt::For(s) => {
                    walk(&s.body, out);
                    walk(&s.orelse, out);
                }
                Stmt::While(s) => {
                    walk(&s.body, out);
                    walk(&s.orelse, out);
                }
                Stmt::With(s) => walk(&s.body, out),
                Stmt::Try(s) => {
                    walk(&s.body, out);
                    for h in &s.handlers {
                        let ast::ExceptHandler::ExceptHandler(h) = h;
                        walk(&h.body, out);
                    }
                    walk(&s.orelse, out);
                    walk(&s.finalbody, out);
                }
                Stmt::Match(s) => {
                    for c in &s.cases {
                        walk(&c.body, out);
                    }
                }
                _ => {}
            }
        }
    }
    walk(body, &mut out);
    out
}

fn collect_target_names(expr: &Expr, out: &mut HashSet<String>) {
    match expr {
        Expr::Name(n) => {
            out.insert(n.id.as_str().to_string());
        }
        Expr::Tuple(t) => {
            for e in &t.elts {
                collect_target_names(e, out);
            }
        }
        Expr::List(l) => {
            for e in &l.elts {
                collect_target_names(e, out);
            }
        }
        Expr::Starred(s) => collect_target_names(&s.value, out),
        _ => {}
    }
}

fn attr_chain_key(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Name(n) => Some(n.id.as_str().to_string()),
        Expr::Attribute(attr) => {
            let head = attr_chain_key(&attr.value)?;
            Some(format!("{head}.{}", attr.attr.as_str()))
        }
        _ => None,
    }
}

impl<'a, 'db> CallCollector<'a, 'db> {
    /// Identify a direct `httpx.AsyncClient(...)` / `aiohttp.ClientSession(...)`
    /// constructor call that does NOT wrap a shared transport/connector/ASGI app.
    pub(crate) fn ephemeral_http_client_ctor_name(
        &mut self,
        call: &ast::ExprCall,
    ) -> Option<&'static str> {
        for kw in &call.arguments.keywords {
            if let Some(arg) = &kw.arg
                && matches!(
                    arg.as_str(),
                    "transport" | "mounts" | "app" | "connector" | "connector_owner"
                )
            {
                return None;
            }
        }
        let q = self.external_qualname(&call.func)?;
        match q.as_str() {
            "httpx.AsyncClient" | "httpx._client.AsyncClient" => Some("httpx.AsyncClient"),
            "aiohttp.ClientSession" | "aiohttp.client.ClientSession" => {
                Some("aiohttp.ClientSession")
            }
            _ => None,
        }
    }

    pub(crate) fn maybe_record_async_with_ephemeral_client(
        &mut self,
        caller: FunctionId,
        item: &ast::WithItem,
    ) {
        if self.dead_depth > 0
            || self.in_lambda
            || self.off_loop_thread_depth > 0
            || self.function(caller).is_installed_third_party()
        {
            return;
        }
        let Expr::Call(call) = &item.context_expr else {
            return;
        };
        let Some(client_name) = self.ephemeral_http_client_ctor_name(call) else {
            return;
        };
        self.clear_pending_call_args();
        self.push_edge(
            caller,
            CalleeRef::External {
                qualname: format!("ephemeral-async-client:{client_name}"),
            },
            call.range(),
            CallKind::WithEnter,
            false,
            Some(false),
        );
    }

    pub(crate) fn note_ephemeral_client_binding(&mut self, name: &str, value: &Expr) {
        self.local_ephemeral_clients.remove(name);
        let Some(caller) = self.current_fn else {
            return;
        };
        if self.function(caller).is_installed_third_party()
            || self.nonlocal_or_global_names.contains(name)
            || self
                .fn_scopes
                .last()
                .is_some_and(|s| s.returned.contains(name))
        {
            return;
        }
        let Expr::Call(call) = value else {
            return;
        };
        let Some(client_name) = self.ephemeral_http_client_ctor_name(call) else {
            return;
        };
        self.local_ephemeral_clients.insert(
            name.to_string(),
            (client_name, call.range(), self.loop_depth > 0),
        );
    }

    pub(crate) fn maybe_record_local_client_close(
        &mut self,
        caller: FunctionId,
        call: &ast::ExprCall,
    ) {
        if self.local_ephemeral_clients.is_empty()
            || self.function(caller).is_installed_third_party()
        {
            return;
        }
        let Expr::Attribute(attr) = call.func.as_ref() else {
            return;
        };
        if !matches!(attr.attr.as_str(), "aclose" | "close") {
            return;
        }
        let Expr::Name(n) = attr.value.as_ref() else {
            return;
        };
        let Some((client_name, ctor_range, ctor_in_loop)) =
            self.local_ephemeral_clients.remove(n.id.as_str())
        else {
            return;
        };
        let saved_loop = self.loop_depth;
        if !ctor_in_loop {
            self.loop_depth = 0;
        }
        let saved_literals = std::mem::take(&mut self.pending_literals);
        let saved_kw_true = std::mem::take(&mut self.pending_keyword_true);
        let saved_callables = std::mem::take(&mut self.pending_callables);
        let saved_kw_callables = std::mem::take(&mut self.pending_keyword_callables);
        let saved_awaited = self.pending_is_awaited;
        self.pending_is_awaited = false;
        self.push_edge(
            caller,
            CalleeRef::External {
                qualname: format!("ephemeral-async-client:{client_name}"),
            },
            ctor_range,
            CallKind::Call,
            false,
            Some(false),
        );
        self.pending_literals = saved_literals;
        self.pending_keyword_true = saved_kw_true;
        self.pending_callables = saved_callables;
        self.pending_keyword_callables = saved_kw_callables;
        self.pending_is_awaited = saved_awaited;
        self.loop_depth = saved_loop;
    }

    pub(crate) fn note_stateful_session_binding(&mut self, name: &str, value: &Expr) {
        if self.expr_produces_stateful_session(value) {
            self.local_stateful_sessions.insert(name.to_string());
        } else {
            self.local_stateful_sessions.remove(name);
        }
    }

    pub(crate) fn note_async_with_stateful_session(&mut self, item: &ast::WithItem) {
        let Some(Expr::Name(name)) = item.optional_vars.as_deref() else {
            return;
        };
        if self.expr_produces_stateful_session(&item.context_expr)
            || self.expr_has_stateful_session_type(item.optional_vars.as_deref().unwrap())
        {
            self.local_stateful_sessions
                .insert(name.id.as_str().to_string());
        }
    }

    fn expr_produces_stateful_session(&mut self, expr: &Expr) -> bool {
        let inner = match expr {
            Expr::Await(aw) => aw.value.as_ref(),
            other => other,
        };
        if self.expr_has_stateful_session_type(inner) {
            return true;
        }
        let Expr::Call(call) = inner else {
            return false;
        };
        if let Some(q) = self.external_qualname(&call.func)
            && is_stateful_session_factory_call_qual(&q)
        {
            return true;
        }
        if let Some(callee_ty) = self.type_instance_qualname(&call.func)
            && is_sessionmaker_type_qual(&callee_ty)
        {
            return true;
        }
        false
    }

    fn expr_has_stateful_session_type(&self, expr: &Expr) -> bool {
        let Some(ty) = expr.inferred_type(self.model) else {
            return false;
        };
        let env = self.model.program_environment();
        self.type_is_stateful_session(ty, &env)
    }

    fn type_is_stateful_session(
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
                if matches!(elem, Type::Never) || elem.is_none(self.model.db()) {
                    continue;
                }
                saw_arm = true;
                if !self.type_is_stateful_session(*elem, env) {
                    return false;
                }
            }
            return saw_arm;
        }
        let Some(TypeDefinition::StaticClass(def) | TypeDefinition::DynamicClass(def)) =
            ty.definition(self.model.db(), env)
        else {
            return false;
        };
        let Some(name) = def.name(self.model.db()) else {
            return false;
        };
        let Some(module) = module_of_definition(self.model, def) else {
            return false;
        };
        let qual = format!("{module}.{name}");
        is_stateful_async_db_handle_qual(&qual)
    }

    fn expr_stateful_session_key(
        &self,
        expr: &Expr,
        excluded_names: &HashSet<String>,
    ) -> Option<String> {
        match expr {
            Expr::Name(n) => {
                let id = n.id.as_str();
                if excluded_names.contains(id) {
                    return None;
                }
                if self.local_stateful_sessions.contains(id)
                    || self.expr_has_stateful_session_type(expr)
                {
                    return Some(id.to_string());
                }
                None
            }
            Expr::Attribute(_) => {
                let key = attr_chain_key(expr)?;
                let root = key.split('.').next().unwrap_or(&key);
                if excluded_names.contains(root) {
                    return None;
                }
                self.expr_has_stateful_session_type(expr).then_some(key)
            }
            _ => None,
        }
    }

    /// Collect stateful session keys referenced by a concurrent task expression
    /// without crossing `await` (already evaluated sequentially) or lambdas.
    fn collect_task_expr_sessions(
        &self,
        expr: &Expr,
        excluded_names: &HashSet<String>,
        out: &mut HashSet<String>,
    ) {
        if let Some(key) = self.expr_stateful_session_key(expr, excluded_names) {
            out.insert(key);
            return;
        }
        match expr {
            Expr::Await(_) | Expr::Lambda(_) => {}
            Expr::Call(call) => {
                if let Expr::Attribute(attr) = call.func.as_ref() {
                    self.collect_task_expr_sessions(&attr.value, excluded_names, out);
                }
                for arg in &call.arguments.args {
                    self.collect_task_expr_sessions(arg, excluded_names, out);
                }
                for kw in &call.arguments.keywords {
                    self.collect_task_expr_sessions(&kw.value, excluded_names, out);
                }
            }
            Expr::Attribute(attr) => {
                self.collect_task_expr_sessions(&attr.value, excluded_names, out);
            }
            Expr::Tuple(t) => {
                for e in &t.elts {
                    self.collect_task_expr_sessions(e, excluded_names, out);
                }
            }
            Expr::List(l) => {
                for e in &l.elts {
                    self.collect_task_expr_sessions(e, excluded_names, out);
                }
            }
            Expr::If(ifexp) => {
                self.collect_task_expr_sessions(&ifexp.body, excluded_names, out);
                self.collect_task_expr_sessions(&ifexp.orelse, excluded_names, out);
            }
            _ => {}
        }
    }

    pub(crate) fn maybe_record_gather_shared_session(
        &mut self,
        caller: FunctionId,
        call: &ast::ExprCall,
        func_ext_qual: Option<&str>,
    ) {
        if !matches!(
            func_ext_qual,
            Some("asyncio.gather" | "asyncio.tasks.gather")
        ) {
            return;
        }
        if self.function(caller).is_installed_third_party() {
            return;
        }
        let mut counts: BTreeMap<String, u32> = BTreeMap::new();
        let empty_excl = HashSet::new();

        for arg in &call.arguments.args {
            match arg {
                Expr::Starred(starred) => {
                    let (elt, generators): (&Expr, &[ast::Comprehension]) =
                        match starred.value.as_ref() {
                            Expr::ListComp(lc) => (&lc.elt, &lc.generators),
                            Expr::Generator(genexpr) => (&genexpr.elt, &genexpr.generators),
                            Expr::SetComp(sc) => (&sc.elt, &sc.generators),
                            _ => continue,
                        };
                    let mut comp_bound = HashSet::new();
                    for comp in generators {
                        collect_target_names(&comp.target, &mut comp_bound);
                    }
                    let mut task_sessions = HashSet::new();
                    self.collect_task_expr_sessions(elt, &comp_bound, &mut task_sessions);
                    for k in task_sessions {
                        *counts.entry(k).or_insert(0) += 2;
                    }
                }
                Expr::Await(_) => {}
                other => {
                    let mut task_sessions = HashSet::new();
                    self.collect_task_expr_sessions(other, &empty_excl, &mut task_sessions);
                    for k in task_sessions {
                        *counts.entry(k).or_insert(0) += 1;
                    }
                }
            }
        }

        let shared: Vec<String> = counts
            .into_iter()
            .filter_map(|(k, c)| (c >= 2).then_some(k))
            .collect();
        if shared.is_empty() {
            return;
        }
        let saved_literals = std::mem::take(&mut self.pending_literals);
        let saved_kw_true = std::mem::take(&mut self.pending_keyword_true);
        let saved_callables = std::mem::take(&mut self.pending_callables);
        let saved_kw_callables = std::mem::take(&mut self.pending_keyword_callables);
        let saved_awaited = self.pending_is_awaited;
        self.pending_is_awaited = false;
        for handle in shared {
            self.push_edge(
                caller,
                CalleeRef::External {
                    qualname: format!("shared-async-session:{handle}"),
                },
                call.range(),
                CallKind::Call,
                false,
                Some(false),
            );
        }
        self.pending_literals = saved_literals;
        self.pending_keyword_true = saved_kw_true;
        self.pending_callables = saved_callables;
        self.pending_keyword_callables = saved_kw_callables;
        self.pending_is_awaited = saved_awaited;
    }

    pub(crate) fn maybe_record_taskgroup_shared_session(
        &mut self,
        caller: FunctionId,
        with_stmt: &ast::StmtWith,
    ) {
        if !with_stmt.is_async
            || self.dead_depth > 0
            || self.in_lambda
            || self.off_loop_thread_depth > 0
            || self.function(caller).is_installed_third_party()
        {
            return;
        }
        for item in &with_stmt.items {
            let Some(Expr::Name(tg_var)) = item.optional_vars.as_deref() else {
                continue;
            };
            let Expr::Call(call) = &item.context_expr else {
                continue;
            };
            let Some(q) = self.external_qualname(&call.func) else {
                continue;
            };
            if !is_task_group_cm_qual(&q) {
                continue;
            }
            let tg_name = tg_var.id.as_str();
            let mut counts: BTreeMap<String, u32> = BTreeMap::new();
            let mut loop_bound = HashSet::new();
            self.scan_taskgroup_stmts(
                &with_stmt.body,
                tg_name,
                false,
                &mut loop_bound,
                &mut counts,
            );
            for (handle, count) in counts {
                if count >= 2 {
                    self.clear_pending_call_args();
                    self.push_edge(
                        caller,
                        CalleeRef::External {
                            qualname: format!("shared-async-session:{handle}"),
                        },
                        item.context_expr.range(),
                        CallKind::WithEnter,
                        false,
                        Some(false),
                    );
                }
            }
        }
    }

    fn scan_taskgroup_stmts(
        &self,
        stmts: &[Stmt],
        tg_name: &str,
        in_loop: bool,
        loop_bound: &mut HashSet<String>,
        counts: &mut BTreeMap<String, u32>,
    ) {
        for stmt in stmts {
            match stmt {
                Stmt::Assign(a) if in_loop => {
                    for t in &a.targets {
                        collect_target_names(t, loop_bound);
                    }
                    self.scan_taskgroup_expr(&a.value, tg_name, in_loop, loop_bound, counts);
                }
                Stmt::AnnAssign(ann) if in_loop => {
                    collect_target_names(&ann.target, loop_bound);
                    if let Some(v) = &ann.value {
                        self.scan_taskgroup_expr(v, tg_name, in_loop, loop_bound, counts);
                    }
                }
                Stmt::Assign(a) => {
                    self.scan_taskgroup_expr(&a.value, tg_name, in_loop, loop_bound, counts);
                }
                Stmt::AnnAssign(ann) => {
                    if let Some(v) = &ann.value {
                        self.scan_taskgroup_expr(v, tg_name, in_loop, loop_bound, counts);
                    }
                }
                Stmt::Expr(e) => {
                    self.scan_taskgroup_expr(&e.value, tg_name, in_loop, loop_bound, counts);
                }
                Stmt::Return(r) => {
                    if let Some(v) = &r.value {
                        self.scan_taskgroup_expr(v, tg_name, in_loop, loop_bound, counts);
                    }
                }
                Stmt::If(s) => {
                    self.scan_taskgroup_stmts(&s.body, tg_name, in_loop, loop_bound, counts);
                    for c in &s.elif_else_clauses {
                        self.scan_taskgroup_stmts(&c.body, tg_name, in_loop, loop_bound, counts);
                    }
                }
                Stmt::For(s) => {
                    let mut inner_bound = loop_bound.clone();
                    collect_target_names(&s.target, &mut inner_bound);
                    self.scan_taskgroup_stmts(&s.body, tg_name, true, &mut inner_bound, counts);
                }
                Stmt::While(s) => {
                    let mut inner_bound = loop_bound.clone();
                    self.scan_taskgroup_stmts(&s.body, tg_name, true, &mut inner_bound, counts);
                }
                Stmt::With(s) => {
                    let mut inner_bound = loop_bound.clone();
                    if in_loop {
                        for item in &s.items {
                            if let Some(vars) = &item.optional_vars {
                                collect_target_names(vars, &mut inner_bound);
                            }
                        }
                    }
                    self.scan_taskgroup_stmts(&s.body, tg_name, in_loop, &mut inner_bound, counts);
                }
                Stmt::Try(s) => {
                    self.scan_taskgroup_stmts(&s.body, tg_name, in_loop, loop_bound, counts);
                    for h in &s.handlers {
                        let ast::ExceptHandler::ExceptHandler(h) = h;
                        self.scan_taskgroup_stmts(&h.body, tg_name, in_loop, loop_bound, counts);
                    }
                    self.scan_taskgroup_stmts(&s.orelse, tg_name, in_loop, loop_bound, counts);
                    self.scan_taskgroup_stmts(&s.finalbody, tg_name, in_loop, loop_bound, counts);
                }
                Stmt::Match(s) => {
                    for c in &s.cases {
                        self.scan_taskgroup_stmts(&c.body, tg_name, in_loop, loop_bound, counts);
                    }
                }
                _ => {}
            }
        }
    }

    fn scan_taskgroup_expr(
        &self,
        expr: &Expr,
        tg_name: &str,
        in_loop: bool,
        loop_bound: &HashSet<String>,
        counts: &mut BTreeMap<String, u32>,
    ) {
        // Immediate `await tg.create_task(...)` runs sequentially — do not count as concurrent.
        if matches!(expr, Expr::Await(_) | Expr::Lambda(_)) {
            return;
        }
        if let Expr::Call(call) = expr
            && let Expr::Attribute(attr) = call.func.as_ref()
            && matches!(attr.value.as_ref(), Expr::Name(n) if n.id.as_str() == tg_name)
        {
            let mut task_sessions = HashSet::new();
            match attr.attr.as_str() {
                "create_task" => {
                    if let Some(coro) = call.arguments.args.first() {
                        self.collect_task_expr_sessions(coro, loop_bound, &mut task_sessions);
                    }
                }
                "start_soon" => {
                    for arg in call.arguments.args.iter().skip(1) {
                        self.collect_task_expr_sessions(arg, loop_bound, &mut task_sessions);
                    }
                }
                _ => {}
            }
            let weight = if in_loop { 2 } else { 1 };
            for k in task_sessions {
                *counts.entry(k).or_insert(0) += weight;
            }
            return;
        }
        match expr {
            Expr::Call(call) => {
                for arg in &call.arguments.args {
                    self.scan_taskgroup_expr(arg, tg_name, in_loop, loop_bound, counts);
                }
                for kw in &call.arguments.keywords {
                    self.scan_taskgroup_expr(&kw.value, tg_name, in_loop, loop_bound, counts);
                }
            }
            Expr::Tuple(t) => {
                for e in &t.elts {
                    self.scan_taskgroup_expr(e, tg_name, in_loop, loop_bound, counts);
                }
            }
            Expr::List(l) => {
                for e in &l.elts {
                    self.scan_taskgroup_expr(e, tg_name, in_loop, loop_bound, counts);
                }
            }
            Expr::ListComp(lc) => {
                let mut inner_bound = loop_bound.clone();
                for comp in &lc.generators {
                    collect_target_names(&comp.target, &mut inner_bound);
                }
                self.scan_taskgroup_expr(&lc.elt, tg_name, true, &inner_bound, counts);
            }
            _ => {}
        }
    }
}
