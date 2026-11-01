//! Off-loop thread-identity guards and Future.done() structural matching.

use ruff_python_ast::{self as ast, Expr};

/// `x != threading.get_ident()` / `current_thread() is not main_thread()` — body is off-loop.
///
/// For `get_ident()` compares, the other operand must look like a loop-thread
/// identity (`hass.loop_thread_id`, `self._loop_thread_id`, …) — not an arbitrary
/// owner/id field (`lk.owner != get_ident()` stays on-loop / unguarded).
pub(super) fn is_off_loop_thread_test(test: &Expr) -> bool {
    match test {
        Expr::UnaryOp(u) if matches!(u.op, ast::UnaryOp::Not) => {
            is_on_loop_thread_test(u.operand.as_ref())
        }
        Expr::Compare(cmp) if cmp.ops.len() == 1 && cmp.operands.len() == 2 => {
            let op = cmp.ops[0];
            let left = &cmp.operands[0];
            let right = &cmp.operands[1];
            match op {
                ast::CmpOp::NotEq | ast::CmpOp::IsNot => {
                    get_ident_vs_loop_thread_id(left, right)
                        || (is_current_thread_call(left) && is_main_thread_call(right))
                        || (is_main_thread_call(left) && is_current_thread_call(right))
                }
                _ => false,
            }
        }
        _ => false,
    }
}

/// `x == threading.get_ident()` / `current_thread() is main_thread()` — then is on-loop;
/// the complementary else branch is off-loop.
pub(super) fn is_on_loop_thread_test(test: &Expr) -> bool {
    match test {
        Expr::Compare(cmp) if cmp.ops.len() == 1 && cmp.operands.len() == 2 => {
            let op = cmp.ops[0];
            let left = &cmp.operands[0];
            let right = &cmp.operands[1];
            match op {
                ast::CmpOp::Eq | ast::CmpOp::Is => {
                    get_ident_vs_loop_thread_id(left, right)
                        || (is_current_thread_call(left) && is_main_thread_call(right))
                        || (is_main_thread_call(left) && is_current_thread_call(right))
                }
                _ => false,
            }
        }
        _ => false,
    }
}

fn get_ident_vs_loop_thread_id(left: &Expr, right: &Expr) -> bool {
    (is_get_ident_call(left) && is_loop_thread_identity(right))
        || (is_get_ident_call(right) && is_loop_thread_identity(left))
}

/// Name/attr path containing `loop` and (`thread` or `ident`), e.g. `loop_thread_id`.
fn is_loop_thread_identity(expr: &Expr) -> bool {
    let Some(path) = expr_attr_path(expr) else {
        return false;
    };
    let lower = path.to_ascii_lowercase();
    lower.contains("loop") && (lower.contains("thread") || lower.contains("ident"))
}

fn is_get_ident_call(expr: &Expr) -> bool {
    match expr {
        Expr::Call(c) => match c.func.as_ref() {
            Expr::Attribute(a) if a.attr.as_str() == "get_ident" => {
                attr_root_is_threading(&a.value)
            }
            Expr::Name(n) if n.id.as_str() == "get_ident" => true,
            _ => false,
        },
        _ => false,
    }
}

fn is_current_thread_call(expr: &Expr) -> bool {
    match expr {
        Expr::Call(c) => match c.func.as_ref() {
            Expr::Attribute(a) if a.attr.as_str() == "current_thread" => {
                attr_root_is_threading(&a.value)
            }
            Expr::Name(n) if n.id.as_str() == "current_thread" => true,
            _ => false,
        },
        _ => false,
    }
}

fn is_main_thread_call(expr: &Expr) -> bool {
    match expr {
        Expr::Call(c) => match c.func.as_ref() {
            Expr::Attribute(a) if a.attr.as_str() == "main_thread" => {
                attr_root_is_threading(&a.value)
            }
            Expr::Name(n) if n.id.as_str() == "main_thread" => true,
            _ => false,
        },
        _ => false,
    }
}

fn attr_root_is_threading(expr: &Expr) -> bool {
    match expr {
        Expr::Name(n) => {
            let s = n.id.as_str();
            s == "threading" || s == "_thread" || s == "thread"
        }
        Expr::Attribute(a) => attr_root_is_threading(&a.value),
        _ => false,
    }
}

/// Structural path for a name/attribute chain (`fut`, `self.x`, `a.b`).
pub(super) fn expr_attr_path(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Name(n) => Some(n.id.as_str().to_string()),
        Expr::Attribute(a) => {
            let base = expr_attr_path(&a.value)?;
            Some(format!("{base}.{}", a.attr.as_str()))
        }
        _ => None,
    }
}

/// Receivers proven `.done()` in a test / conjunct (`if fut.done():`, `a.b.done() and …`).
pub(super) fn futures_proven_done_in_test(test: &Expr) -> Vec<String> {
    let mut out = Vec::new();
    collect_done_receivers(test, &mut out);
    out
}

fn collect_done_receivers(test: &Expr, out: &mut Vec<String>) {
    match test {
        Expr::BoolOp(b) if matches!(b.op, ast::BoolOp::And) => {
            for v in &b.values {
                collect_done_receivers(v, out);
            }
        }
        Expr::Call(c) => {
            if let Expr::Attribute(a) = c.func.as_ref()
                && a.attr.as_str() == "done"
                && c.arguments.args.is_empty()
                && c.arguments.keywords.is_empty()
                && let Some(path) = expr_attr_path(&a.value)
            {
                out.push(path);
            }
        }
        _ => {}
    }
}
