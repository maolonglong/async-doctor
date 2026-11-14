//! Proven non-blocking variants of threading / queue / socket / `:memory:` SQLite sinks.
//!
//! - `acquire(blocking=False)`, `join(timeout=0)`, `put`/`get(block=False)`,
//!   `put_nowait` / `get_nowait`, and `put` on an unbounded `queue.Queue()`
//! - `sqlite3.connect(":memory:")` and `uri=True` with `mode=memory` (plus `.cursor()`)
//! - `sock.setblocking(False)` / `sock.settimeout(0 | 0.0)` non-blocking socket ops

use ruff_python_ast::{self as ast, Expr};

use super::CallCollector;

impl<'a, 'db> CallCollector<'a, 'db> {
    /// True when this external call is a threading/queue/sqlite/socket sink that cannot block.
    pub(super) fn is_proven_nonblocking_lock_call(
        &mut self,
        qualname: &str,
        call: &ast::ExprCall,
    ) -> bool {
        if self.call_is_in_memory_sqlite(qualname, call) {
            return true;
        }
        if self.call_is_nonblocking_socket_op(qualname, call) {
            return true;
        }
        let method = qualname
            .rsplit_once('.')
            .map(|(_, m)| m)
            .unwrap_or(qualname);
        match method {
            "acquire" if is_lock_or_semaphore_acquire(qualname) => acquire_is_nonblocking(call),
            "join" if is_thread_join(qualname) => join_timeout_is_zero(call),
            "get" | "put" if is_queue_get_or_put(qualname) => {
                queue_call_is_nonblocking(call)
                    || (method == "put" && self.receiver_is_unbounded_queue(call))
            }
            "get_nowait" | "put_nowait" if is_queue_family(qualname) => true,
            _ => false,
        }
    }

    /// Update `nonblocking_socket_vars` when `sock.setblocking(...)` or
    /// `sock.settimeout(...)` is called on a local socket variable.
    pub(super) fn note_socket_mode_call(
        &mut self,
        call: &ast::ExprCall,
        func_ext_qual: Option<&str>,
    ) {
        let Expr::Attribute(attr) = call.func.as_ref() else {
            return;
        };
        let method = attr.attr.as_str();
        if !matches!(method, "setblocking" | "settimeout") {
            return;
        }
        let Expr::Name(recv_name) = attr.value.as_ref() else {
            return;
        };
        let var = recv_name.id.as_str();
        let has_no_kwargs = call.arguments.keywords.is_empty();
        let first_arg = (call.arguments.args.len() == 1).then(|| &call.arguments.args[0]);
        let proven_nonblocking = match (method, first_arg, has_no_kwargs) {
            ("setblocking", Some(arg), true) => expr_is_falsey_blocking_flag(arg),
            ("settimeout", Some(arg), true) => expr_is_zero(arg),
            _ => false,
        };
        if proven_nonblocking && self.is_socket_receiver_or_qual(&attr.value, func_ext_qual) {
            self.nonblocking_socket_vars.insert(var.to_string());
        } else {
            self.nonblocking_socket_vars.remove(var);
        }
    }

    fn is_socket_receiver_or_qual(&self, recv: &Expr, func_ext_qual: Option<&str>) -> bool {
        if let Some(q) = func_ext_qual
            && (q.starts_with("socket.") || q.starts_with("_socket.") || q.starts_with("ssl."))
        {
            return true;
        }
        self.type_instance_qualname(recv).is_some_and(|ty| {
            ty.starts_with("socket.") || ty.starts_with("_socket.") || ty.starts_with("ssl.")
        })
    }

    fn call_is_nonblocking_socket_op(&self, qualname: &str, call: &ast::ExprCall) -> bool {
        if !(qualname.starts_with("socket.")
            || qualname.starts_with("_socket.")
            || qualname.starts_with("ssl."))
        {
            return false;
        }
        let Some((_, method)) = qualname.rsplit_once('.') else {
            return false;
        };
        if !matches!(
            method,
            "recv"
                | "recv_into"
                | "send"
                | "connect"
                | "accept"
                | "sendto"
                | "recvfrom"
                | "recvfrom_into"
        ) {
            return false;
        }
        let Expr::Attribute(attr) = call.func.as_ref() else {
            return false;
        };
        matches!(
            attr.value.as_ref(),
            Expr::Name(n) if self.nonblocking_socket_vars.contains(n.id.as_str())
        )
    }

    pub(super) fn note_socket_binding(&mut self, name: &str, value: &Expr) {
        if let Expr::Name(src) = value
            && self.nonblocking_socket_vars.contains(src.id.as_str())
        {
            self.nonblocking_socket_vars.insert(name.to_string());
        } else {
            self.nonblocking_socket_vars.remove(name);
        }
    }

    pub(super) fn note_sqlite_binding(&mut self, name: &str, value: &Expr) {
        if self.expr_produces_in_memory_sqlite(value) {
            self.in_memory_sqlite_vars.insert(name.to_string());
        } else {
            self.in_memory_sqlite_vars.remove(name);
        }
    }

    fn expr_produces_in_memory_sqlite(&mut self, expr: &Expr) -> bool {
        match expr {
            Expr::Name(n) => self.in_memory_sqlite_vars.contains(n.id.as_str()),
            Expr::Call(call) => {
                let q = self.external_qualname(&call.func);
                if is_sqlite_connect_call(call, q.as_deref()) && sqlite_connect_is_in_memory(call) {
                    return true;
                }
                if let Expr::Attribute(attr) = call.func.as_ref()
                    && matches!(
                        attr.attr.as_str(),
                        "cursor" | "execute" | "executemany" | "executescript"
                    )
                {
                    return self.expr_produces_in_memory_sqlite(&attr.value);
                }
                false
            }
            _ => false,
        }
    }

    fn call_is_in_memory_sqlite(&mut self, qualname: &str, call: &ast::ExprCall) -> bool {
        if !(qualname.starts_with("sqlite3.") || qualname.starts_with("_sqlite3.")) {
            return false;
        }
        if is_sqlite_connect_call(call, Some(qualname)) && sqlite_connect_is_in_memory(call) {
            return true;
        }
        let Expr::Attribute(attr) = call.func.as_ref() else {
            return false;
        };
        self.expr_produces_in_memory_sqlite(&attr.value)
    }

    pub(super) fn note_queue_binding(&mut self, name: &str, value: &Expr) {
        match queue_ctor_boundness(value) {
            Some(true) => {
                self.unbounded_queues.insert(name.to_string());
            }
            Some(false) => {
                self.unbounded_queues.remove(name);
            }
            None => match value {
                // Transfer only when the RHS name is itself proven unbounded.
                Expr::Name(n) if self.unbounded_queues.contains(n.id.as_str()) => {
                    self.unbounded_queues.insert(name.to_string());
                }
                // Rebind to a bare/other name (or any non-Queue expr) clears status.
                _ => {
                    self.unbounded_queues.remove(name);
                }
            },
        }
    }

    fn receiver_is_unbounded_queue(&self, call: &ast::ExprCall) -> bool {
        let Expr::Attribute(attr) = call.func.as_ref() else {
            return false;
        };
        match attr.value.as_ref() {
            Expr::Name(n) => self.unbounded_queues.contains(n.id.as_str()),
            _ => false,
        }
    }
}

fn is_sqlite_connect_call(call: &ast::ExprCall, ext_qual: Option<&str>) -> bool {
    if let Some(q) = ext_qual
        && matches!(
            q,
            "sqlite3.connect" | "sqlite3.dbapi2.connect" | "_sqlite3.connect"
        )
    {
        return true;
    }
    matches!(
        call.func.as_ref(),
        Expr::Attribute(a)
            if a.attr.as_str() == "connect"
                && matches!(a.value.as_ref(), Expr::Name(n) if n.id.as_str() == "sqlite3")
    )
}

fn sqlite_connect_is_in_memory(call: &ast::ExprCall) -> bool {
    if call.arguments.keywords.iter().any(|kw| kw.arg.is_none()) {
        return false;
    }
    let db_arg = call.arguments.args.first().or_else(|| {
        call.arguments
            .keywords
            .iter()
            .find(|kw| kw.arg.as_ref().is_some_and(|a| a.as_str() == "database"))
            .map(|kw| &kw.value)
    });
    let Some(Expr::StringLiteral(s)) = db_arg else {
        return false;
    };
    let db_str = s.value.to_str();
    if db_str == ":memory:" {
        return true;
    }
    let uri_true = call.arguments.keywords.iter().any(|kw| {
        kw.arg.as_ref().is_some_and(|a| a.as_str() == "uri")
            && matches!(&kw.value, Expr::BooleanLiteral(b) if b.value)
    });
    if !uri_true {
        return false;
    }
    if db_str.starts_with("file::memory:") {
        return true;
    }
    db_str.starts_with("file:")
        && db_str
            .split_once('?')
            .is_some_and(|(_, query)| query.split('&').any(|kv| kv == "mode=memory"))
}

fn is_lock_or_semaphore_acquire(q: &str) -> bool {
    let lower = q.to_ascii_lowercase();
    if !lower.ends_with(".acquire") {
        return false;
    }
    lower.starts_with("threading.")
        || lower.starts_with("_thread.")
        || lower.starts_with("multiprocessing.")
}

fn is_thread_join(q: &str) -> bool {
    q.ends_with("Thread.join") || (q.ends_with(".join") && q.contains("Thread"))
}

fn is_queue_get_or_put(q: &str) -> bool {
    q.ends_with("Queue.get")
        || q.ends_with("Queue.put")
        || q.ends_with("SimpleQueue.get")
        || q.ends_with("SimpleQueue.put")
}

fn is_queue_family(q: &str) -> bool {
    q.contains("Queue.") || q.contains("SimpleQueue.")
}

fn acquire_is_nonblocking(call: &ast::ExprCall) -> bool {
    if let Some(arg) = call.arguments.args.first()
        && expr_is_falsey_blocking_flag(arg)
    {
        return true;
    }
    for kw in &call.arguments.keywords {
        let Some(name) = kw.arg.as_ref() else {
            continue;
        };
        if matches!(name.as_str(), "blocking" | "block") && expr_is_falsey_blocking_flag(&kw.value)
        {
            return true;
        }
        if name.as_str() == "timeout" && expr_is_zero(&kw.value) {
            return true;
        }
    }
    false
}

fn join_timeout_is_zero(call: &ast::ExprCall) -> bool {
    if let Some(arg) = call.arguments.args.first()
        && expr_is_zero(arg)
    {
        return true;
    }
    for kw in &call.arguments.keywords {
        if kw.arg.as_ref().is_some_and(|a| a.as_str() == "timeout") && expr_is_zero(&kw.value) {
            return true;
        }
    }
    false
}

fn queue_call_is_nonblocking(call: &ast::ExprCall) -> bool {
    let func_is_put = matches!(
        call.func.as_ref(),
        Expr::Attribute(a) if a.attr.as_str() == "put"
    );
    if func_is_put {
        if let Some(arg) = call.arguments.args.get(1)
            && expr_is_falsey_blocking_flag(arg)
        {
            return true;
        }
    } else if let Some(arg) = call.arguments.args.first()
        && expr_is_falsey_blocking_flag(arg)
    {
        return true;
    }
    for kw in &call.arguments.keywords {
        let Some(name) = kw.arg.as_ref() else {
            continue;
        };
        match name.as_str() {
            "block" if expr_is_falsey_blocking_flag(&kw.value) => return true,
            "timeout" if expr_is_zero(&kw.value) => return true,
            _ => {}
        }
    }
    false
}

fn expr_is_falsey_blocking_flag(expr: &Expr) -> bool {
    match expr {
        Expr::BooleanLiteral(b) => !b.value,
        Expr::NumberLiteral(n) => match &n.value {
            ast::Number::Int(i) => i.as_u64() == Some(0) || i.as_i64() == Some(0),
            _ => false,
        },
        _ => false,
    }
}

fn expr_is_zero(expr: &Expr) -> bool {
    match expr {
        Expr::NumberLiteral(n) => match &n.value {
            ast::Number::Int(i) => i.as_u64() == Some(0) || i.as_i64() == Some(0),
            ast::Number::Float(f) => *f == 0.0,
            _ => false,
        },
        Expr::UnaryOp(u) if matches!(u.op, ast::UnaryOp::USub | ast::UnaryOp::Invert) => false,
        _ => false,
    }
}

/// `Some(true)` = unbounded Queue ctor; `Some(false)` = bounded; `None` = not a Queue().
fn queue_ctor_boundness(value: &Expr) -> Option<bool> {
    let Expr::Call(call) = value else {
        return None;
    };
    if !is_queue_constructor(&call.func) {
        return None;
    }
    if call.arguments.args.is_empty()
        && call
            .arguments
            .keywords
            .iter()
            .all(|kw| kw.arg.as_ref().is_none_or(|a| a.as_str() != "maxsize"))
    {
        return Some(true);
    }
    if let Some(arg) = call.arguments.args.first() {
        return Some(maxsize_is_unbounded(arg));
    }
    for kw in &call.arguments.keywords {
        if kw.arg.as_ref().is_some_and(|a| a.as_str() == "maxsize") {
            return Some(maxsize_is_unbounded(&kw.value));
        }
    }
    Some(true)
}

fn is_queue_constructor(func: &Expr) -> bool {
    match func {
        Expr::Name(n) => matches!(n.id.as_str(), "Queue" | "SimpleQueue"),
        Expr::Attribute(attr) => {
            matches!(attr.attr.as_str(), "Queue" | "SimpleQueue")
                && matches!(
                    attr.value.as_ref(),
                    Expr::Name(n) if matches!(n.id.as_str(), "queue" | "multiprocessing")
                )
        }
        _ => false,
    }
}

/// `maxsize <= 0` means unbounded in [`queue.Queue`].
fn maxsize_is_unbounded(expr: &Expr) -> bool {
    match expr {
        Expr::NumberLiteral(n) => match &n.value {
            ast::Number::Int(i) => i.as_i64().is_some_and(|v| v <= 0) || i.as_u64() == Some(0),
            _ => false,
        },
        Expr::UnaryOp(u) if matches!(u.op, ast::UnaryOp::USub) => {
            matches!(
                u.operand.as_ref(),
                Expr::NumberLiteral(n) if matches!(&n.value, ast::Number::Int(i) if i.as_u64().is_some_and(|v| v > 0))
            )
        }
        _ => false,
    }
}
