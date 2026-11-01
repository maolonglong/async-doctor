//! Proven non-blocking variants of threading / queue sinks.
//!
//! `acquire(blocking=False)`, `join(timeout=0)`, `put`/`get(block=False)`,
//! `put_nowait` / `get_nowait`, and `put` on an unbounded `queue.Queue()` must
//! not seed `blocking-lock`.

use ruff_python_ast::{self as ast, Expr};

use super::CallCollector;

impl<'a, 'db> CallCollector<'a, 'db> {
    /// True when this external call is a threading/queue sink that cannot block.
    pub(super) fn is_proven_nonblocking_lock_call(
        &self,
        qualname: &str,
        call: &ast::ExprCall,
    ) -> bool {
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

fn is_lock_or_semaphore_acquire(q: &str) -> bool {
    // Only stdlib / multiprocessing primitives — not redis.lock / filelock / etc.
    // ty may surface `_thread.lock.acquire` / `LockType.acquire` / `threading.Lock.acquire`.
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
    // Positional: acquire(False) / acquire(0) — first arg is `blocking`.
    if let Some(arg) = call.arguments.args.first()
        && expr_is_falsey_blocking_flag(arg)
    {
        return true;
    }
    for kw in &call.arguments.keywords {
        let Some(name) = kw.arg.as_ref() else {
            continue;
        };
        // threading uses `blocking=`; multiprocessing.Lock uses `block=`.
        if matches!(name.as_str(), "blocking" | "block") && expr_is_falsey_blocking_flag(&kw.value)
        {
            return true;
        }
        // acquire(timeout=0) returns immediately (False) without blocking.
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
    // get(False) / put(item, False) — `block` is first kw-only-ish positional
    // after the item for put: put(item, block=True, timeout=None).
    let func_is_put = matches!(
        call.func.as_ref(),
        Expr::Attribute(a) if a.attr.as_str() == "put"
    );
    if func_is_put {
        // put(item, block) — second positional.
        if let Some(arg) = call.arguments.args.get(1)
            && expr_is_falsey_blocking_flag(arg)
        {
            return true;
        }
    } else if let Some(arg) = call.arguments.args.first()
        && expr_is_falsey_blocking_flag(arg)
    {
        // get(False)
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
        Expr::UnaryOp(u) if matches!(u.op, ast::UnaryOp::USub | ast::UnaryOp::Invert) => {
            // timeout=-0 still zero-ish; treat bare 0 only for join.
            false
        }
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
    // queue.Queue() / Queue() — no maxsize → unbounded.
    if call.arguments.args.is_empty()
        && call
            .arguments
            .keywords
            .iter()
            .all(|kw| kw.arg.as_ref().is_none_or(|a| a.as_str() != "maxsize"))
    {
        return Some(true);
    }
    // Positional maxsize.
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
            // maxsize=-1 → unbounded
            matches!(
                u.operand.as_ref(),
                Expr::NumberLiteral(n) if matches!(&n.value, ast::Number::Int(i) if i.as_u64().is_some_and(|v| v > 0))
            )
        }
        _ => false,
    }
}
