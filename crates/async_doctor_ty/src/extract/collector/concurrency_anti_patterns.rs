//! Detection of async client/session lifecycle & concurrency anti-patterns:
//! - `ephemeral-async-client` (performance advice): per-route / per-iteration
//!   construction + teardown of `httpx.AsyncClient` / `aiohttp.ClientSession`.
//! - `shared-async-session` (correctness advice): see `shared_session` submodule.

mod shared_session;

use std::collections::HashSet;

use async_doctor_core::{CallKind, CalleeRef, FunctionId};
use ruff_python_ast::{self as ast, Expr, Stmt};
use ruff_text_size::Ranged;

use super::CallCollector;

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

pub(super) fn collect_target_names(expr: &Expr, out: &mut HashSet<String>) {
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

pub(super) fn attr_chain_key(expr: &Expr) -> Option<String> {
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
    /// Identify a direct `httpx.AsyncClient(...)` / `aiohttp.ClientSession(...)` /
    /// `openai.AsyncOpenAI(...)` / `anthropic.AsyncAnthropic(...)` constructor call
    /// that does NOT wrap a shared transport/connector/ASGI app/http_client.
    pub(crate) fn ephemeral_http_client_ctor_name(
        &mut self,
        call: &ast::ExprCall,
    ) -> Option<&'static str> {
        for kw in &call.arguments.keywords {
            if let Some(arg) = &kw.arg
                && matches!(
                    arg.as_str(),
                    "transport"
                        | "mounts"
                        | "app"
                        | "connector"
                        | "connector_owner"
                        | "http_client"
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
            "openai.AsyncOpenAI" | "openai._client.AsyncOpenAI" => Some("openai.AsyncOpenAI"),
            "anthropic.AsyncAnthropic" | "anthropic._client.AsyncAnthropic" => {
                Some("anthropic.AsyncAnthropic")
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
        let saved_kw_literals = std::mem::take(&mut self.pending_keyword_literals);
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
        self.pending_keyword_literals = saved_kw_literals;
        self.pending_callables = saved_callables;
        self.pending_keyword_callables = saved_kw_callables;
        self.pending_is_awaited = saved_awaited;
        self.loop_depth = saved_loop;
    }
}
