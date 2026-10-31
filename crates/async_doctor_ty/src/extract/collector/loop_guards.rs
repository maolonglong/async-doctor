//! Loop-state guard recognition for nested-event-loop suppression.
//!
//! Covers `if loop is None` / `not is_event_loop_running()`, exception handlers
//! after `get_running_loop`, and idle-loop locals bound via ternary / helpers
//! (litellm `None if is_event_loop_running() else self._idle_reading_loop()`).

use std::collections::HashSet;

use ruff_python_ast::{self as ast, Expr, Stmt};

use super::CallCollector;

impl<'a, 'db> CallCollector<'a, 'db> {
    /// `if idle_loop is not None` / truthy `if idle_loop` after a ternary/helper bind.
    pub(super) fn is_idle_loop_local_guard(&self, test: &Expr) -> bool {
        match test {
            Expr::Name(n) => self.idle_loop_locals.contains(n.id.as_str()),
            Expr::Compare(cmp) if cmp.ops.len() == 1 && cmp.operands.len() == 2 => {
                let op = cmp.ops[0];
                let left = &cmp.operands[0];
                let right = &cmp.operands[1];
                match op {
                    ast::CmpOp::IsNot | ast::CmpOp::NotEq => {
                        (matches!(left, Expr::Name(n) if self.idle_loop_locals.contains(n.id.as_str()))
                            && matches!(right, Expr::NoneLiteral(_)))
                            || (matches!(right, Expr::Name(n) if self.idle_loop_locals.contains(n.id.as_str()))
                                && matches!(left, Expr::NoneLiteral(_)))
                    }
                    _ => false,
                }
            }
            _ => false,
        }
    }

    /// Running-loop test, excluding caller-supplied params named like `is_async`.
    pub(super) fn is_running_loop_test_ctx(&self, test: &Expr) -> bool {
        if let Expr::Name(n) = test
            && is_async_flag_name(n.id.as_str())
        {
            return !self
                .current_param_names
                .iter()
                .any(|p| p.as_str() == n.id.as_str());
        }
        is_running_loop_call(test)
    }

    pub(super) fn is_no_running_loop_test_ctx(&self, test: &Expr) -> bool {
        match test {
            Expr::UnaryOp(u) if matches!(u.op, ast::UnaryOp::Not) => {
                self.is_running_loop_test_ctx(u.operand.as_ref())
            }
            Expr::Compare(cmp) if cmp.ops.len() == 1 && cmp.operands.len() == 2 => {
                // `if main_loop is None:` on a caller-supplied param is not a
                // proven "no running event loop" guard (fixture n2 / n7).
                if compare_is_none_of_param(cmp, &self.current_param_names) {
                    return false;
                }
                is_no_running_loop_test(test)
            }
            _ => is_no_running_loop_test(test),
        }
    }
}

fn compare_is_none_of_param(cmp: &ast::ExprCompare, params: &[String]) -> bool {
    let op = cmp.ops[0];
    if !matches!(op, ast::CmpOp::Is | ast::CmpOp::Eq) {
        return false;
    }
    let left = &cmp.operands[0];
    let right = &cmp.operands[1];
    let name = match (left, right) {
        (Expr::Name(n), Expr::NoneLiteral(_)) | (Expr::NoneLiteral(_), Expr::Name(n)) => {
            n.id.as_str()
        }
        _ => return false,
    };
    params.iter().any(|p| p.as_str() == name)
}

/// Pre-scan a suite so helpers defined later in the same class/module still
/// count when assigned earlier (`_log` before `_idle_reading_loop`).
pub(crate) fn collect_idle_loop_helpers(body: &[Stmt], out: &mut HashSet<String>) {
    for stmt in body {
        match stmt {
            Stmt::FunctionDef(func) => {
                if function_body_returns_idle_loop(&func.body) {
                    out.insert(func.name.as_str().to_string());
                }
                collect_idle_loop_helpers(&func.body, out);
            }
            Stmt::ClassDef(class) => collect_idle_loop_helpers(&class.body, out),
            Stmt::If(if_stmt) => {
                collect_idle_loop_helpers(&if_stmt.body, out);
                for clause in &if_stmt.elif_else_clauses {
                    collect_idle_loop_helpers(&clause.body, out);
                }
            }
            Stmt::With(w) => collect_idle_loop_helpers(&w.body, out),
            Stmt::For(f) => {
                collect_idle_loop_helpers(&f.body, out);
                collect_idle_loop_helpers(&f.orelse, out);
            }
            Stmt::While(w) => {
                collect_idle_loop_helpers(&w.body, out);
                collect_idle_loop_helpers(&w.orelse, out);
            }
            Stmt::Try(t) => {
                collect_idle_loop_helpers(&t.body, out);
                for h in &t.handlers {
                    let ast::ExceptHandler::ExceptHandler(eh) = h;
                    collect_idle_loop_helpers(&eh.body, out);
                }
                collect_idle_loop_helpers(&t.orelse, out);
                collect_idle_loop_helpers(&t.finalbody, out);
            }
            Stmt::Match(m) => {
                for case in &m.cases {
                    collect_idle_loop_helpers(&case.body, out);
                }
            }
            _ => {}
        }
    }
}

/// `if loop is None` / `not loop.is_running()` / `not is_event_loop_running()` / …
pub(super) fn is_no_running_loop_test(test: &Expr) -> bool {
    match test {
        Expr::Compare(cmp) if cmp.ops.len() == 1 && cmp.operands.len() == 2 => {
            let op = cmp.ops[0];
            let left = &cmp.operands[0];
            let right = &cmp.operands[1];
            match op {
                ast::CmpOp::Is | ast::CmpOp::Eq => {
                    (looks_like_loop_name(left) && matches!(right, Expr::NoneLiteral(_)))
                        || (matches!(left, Expr::NoneLiteral(_)) && looks_like_loop_name(right))
                        || (is_running_loop_call(left)
                            && matches!(right, Expr::BooleanLiteral(b) if !b.value))
                        || (matches!(left, Expr::BooleanLiteral(b) if !b.value)
                            && is_running_loop_call(right))
                }
                ast::CmpOp::IsNot | ast::CmpOp::NotEq => {
                    (is_running_loop_call(left)
                        && matches!(right, Expr::BooleanLiteral(b) if b.value))
                        || (matches!(left, Expr::BooleanLiteral(b) if b.value)
                            && is_running_loop_call(right))
                }
                _ => false,
            }
        }
        Expr::UnaryOp(u) if matches!(u.op, ast::UnaryOp::Not) => {
            is_running_loop_test(u.operand.as_ref())
        }
        _ => false,
    }
}

/// `if loop.is_running()` / `is_event_loop_running()` / optional `is_async`.
pub(super) fn is_running_loop_test(test: &Expr) -> bool {
    is_running_loop_call(test) || matches!(test, Expr::Name(n) if is_async_flag_name(n.id.as_str()))
}

fn is_running_loop_call(expr: &Expr) -> bool {
    match expr {
        Expr::Call(call) => match call.func.as_ref() {
            Expr::Attribute(attr) if attr.attr.as_str() == "is_running" => true,
            Expr::Name(n) => is_event_loop_running_helper(n.id.as_str()),
            Expr::Attribute(attr) if is_event_loop_running_helper(attr.attr.as_str()) => true,
            _ => false,
        },
        Expr::Attribute(attr) if attr.attr.as_str() == "is_running" => true,
        _ => false,
    }
}

fn looks_like_loop_name(expr: &Expr) -> bool {
    match expr {
        Expr::Name(n) => {
            let s = n.id.as_str();
            s == "loop" || s.ends_with("_loop") || s.contains("event_loop")
        }
        Expr::Attribute(attr) => {
            let s = attr.attr.as_str();
            s == "loop" || s.ends_with("_loop") || s.contains("event_loop")
        }
        _ => false,
    }
}

fn is_event_loop_running_helper(name: &str) -> bool {
    matches!(
        name,
        "is_event_loop_running"
            | "is_running_event_loop"
            | "event_loop_is_running"
            | "has_running_loop"
            | "running_loop_exists"
    ) || (name.contains("event_loop") && name.contains("running"))
}

fn is_async_flag_name(name: &str) -> bool {
    matches!(name, "is_async" | "in_async" | "async_mode")
}

/// True when `value` binds a loop handle that is only non-None with no running loop.
pub(crate) fn value_is_idle_loop_binding(
    value: &Expr,
    idle_loop_helpers: &HashSet<String>,
) -> bool {
    match value {
        Expr::If(ifexp) => idle_loop_ternary(ifexp, idle_loop_helpers),
        other => is_idle_loop_obtainer(other, idle_loop_helpers),
    }
}

fn idle_loop_ternary(ifexp: &ast::ExprIf, idle_loop_helpers: &HashSet<String>) -> bool {
    let body_none = matches!(ifexp.body.as_ref(), Expr::NoneLiteral(_));
    let orelse_none = matches!(ifexp.orelse.as_ref(), Expr::NoneLiteral(_));
    let body_idle = is_idle_loop_obtainer_in_ternary(ifexp.body.as_ref(), idle_loop_helpers);
    let orelse_idle = is_idle_loop_obtainer_in_ternary(ifexp.orelse.as_ref(), idle_loop_helpers);
    // `None if is_event_loop_running() else <idle_loop | new_event_loop()>`
    if is_running_loop_test(ifexp.test.as_ref()) && body_none && orelse_idle {
        return true;
    }
    // `<idle_loop> if not is_event_loop_running() else None`
    if is_no_running_loop_test(ifexp.test.as_ref()) && body_idle && orelse_none {
        return true;
    }
    false
}

fn is_idle_loop_obtainer(expr: &Expr, idle_loop_helpers: &HashSet<String>) -> bool {
    // Bare `loop = asyncio.new_event_loop()` is not a loop-state guard by itself
    // (fixture n / n4). `new_event_loop` only counts inside a loop-state ternary.
    match expr {
        Expr::Call(call) => {
            let Some(name) = callee_attr_or_name(call.func.as_ref()) else {
                return false;
            };
            // Project helpers: require a body that actually returns an idle/new
            // loop — a name like `create_idle_loop` alone is not enough.
            idle_loop_helpers.contains(name)
        }
        _ => false,
    }
}

fn is_idle_loop_obtainer_in_ternary(expr: &Expr, idle_loop_helpers: &HashSet<String>) -> bool {
    if is_new_event_loop_call(expr) {
        return true;
    }
    match expr {
        Expr::Call(call) => {
            let Some(name) = callee_attr_or_name(call.func.as_ref()) else {
                return false;
            };
            idle_loop_helpers.contains(name)
        }
        _ => false,
    }
}

fn is_new_event_loop_call(expr: &Expr) -> bool {
    let Expr::Call(call) = expr else {
        return false;
    };
    match call.func.as_ref() {
        Expr::Name(n) => n.id.as_str() == "new_event_loop",
        Expr::Attribute(attr) => attr.attr.as_str() == "new_event_loop",
        _ => false,
    }
}

fn callee_attr_or_name(func: &Expr) -> Option<&str> {
    match func {
        Expr::Name(n) => Some(n.id.as_str()),
        Expr::Attribute(attr) => Some(attr.attr.as_str()),
        _ => None,
    }
}

/// Heuristic: function body returns a loop only when idle / not running / new.
pub(crate) fn function_body_returns_idle_loop(body: &[Stmt]) -> bool {
    for stmt in body {
        match stmt {
            Stmt::Return(ret) => {
                if let Some(value) = ret.value.as_ref()
                    && (is_new_event_loop_call(value) || expr_filters_idle_loop(value))
                {
                    return true;
                }
            }
            Stmt::If(if_stmt) => {
                if is_running_loop_test(&if_stmt.test)
                    && body_returns_none_only(&if_stmt.body)
                    && if_stmt
                        .elif_else_clauses
                        .iter()
                        .any(|c| c.test.is_none() && body_returns_non_none(&c.body))
                {
                    return true;
                }
                if function_body_returns_idle_loop(&if_stmt.body) {
                    return true;
                }
                for clause in &if_stmt.elif_else_clauses {
                    if function_body_returns_idle_loop(&clause.body) {
                        return true;
                    }
                }
            }
            _ => {}
        }
    }
    false
}

fn body_returns_none_only(body: &[Stmt]) -> bool {
    matches!(
        body,
        [Stmt::Return(r)] if r.value.as_ref().is_none_or(|v| matches!(v.as_ref(), Expr::NoneLiteral(_)))
    )
}

fn body_returns_non_none(body: &[Stmt]) -> bool {
    body.iter().any(|s| {
        matches!(
            s,
            Stmt::Return(r) if r.value.as_ref().is_some_and(|v| !matches!(v.as_ref(), Expr::NoneLiteral(_)))
        )
    })
}

fn expr_filters_idle_loop(expr: &Expr) -> bool {
    match expr {
        Expr::If(ifexp) => {
            let orelse_none = matches!(ifexp.orelse.as_ref(), Expr::NoneLiteral(_));
            if !orelse_none {
                return false;
            }
            if is_new_event_loop_call(ifexp.body.as_ref()) {
                return is_no_running_loop_test(ifexp.test.as_ref())
                    || test_requires_loop_not_running(ifexp.test.as_ref());
            }
            test_requires_loop_not_running(ifexp.test.as_ref())
                || (looks_like_loop_name(ifexp.body.as_ref())
                    && test_requires_loop_not_running(ifexp.test.as_ref()))
        }
        Expr::BoolOp(b) if matches!(b.op, ast::BoolOp::And) => {
            b.values.iter().any(test_requires_loop_not_running)
                && b.values
                    .iter()
                    .any(|v| looks_like_loop_name(v) || !matches!(v, Expr::UnaryOp(_)))
        }
        _ => false,
    }
}

fn test_requires_loop_not_running(test: &Expr) -> bool {
    match test {
        Expr::BoolOp(b) if matches!(b.op, ast::BoolOp::And) => {
            b.values.iter().any(test_requires_loop_not_running)
        }
        Expr::UnaryOp(u) if matches!(u.op, ast::UnaryOp::Not) => match u.operand.as_ref() {
            Expr::Call(c) => matches!(
                c.func.as_ref(),
                Expr::Attribute(a) if a.attr.as_str() == "is_running"
            ),
            Expr::Attribute(a) if a.attr.as_str() == "is_running" => true,
            other => is_running_loop_call(other),
        },
        other => is_no_running_loop_test(other),
    }
}

pub(super) fn except_is_runtime_error(type_expr: Option<&Expr>) -> bool {
    let Some(expr) = type_expr else {
        return false;
    };
    match expr {
        Expr::Name(n) => n.id.as_str() == "RuntimeError",
        Expr::Attribute(attr) => attr.attr.as_str() == "RuntimeError",
        Expr::Tuple(t) => t.elts.iter().any(|e| except_is_runtime_error(Some(e))),
        _ => false,
    }
}
