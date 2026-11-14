//! Local completion proofs for the actual concurrent Future protocol.

use std::collections::HashSet;

use ruff_python_ast::visitor::{Visitor, walk_expr, walk_pattern, walk_stmt};
use ruff_python_ast::{self as ast, Expr, Pattern, Stmt};
use ty_python_semantic::definitions_for_attribute;

use super::CallCollector;
use super::thread_guards::expr_attr_path;
use crate::extract::helpers::qualname_from_definition;

impl<'a, 'db> CallCollector<'a, 'db> {
    /// Receivers complete on every path producing this truth value.
    pub(super) fn futures_proven_done_in_test(&self, test: &Expr, truth: bool) -> HashSet<String> {
        let mut proven = self.future_test_proofs(test, truth);
        // A test can replace a receiver after checking it, e.g.
        // `future.done() and (future := pending)`. Never reuse that proof.
        let mut writes = ReceiverWrites::default();
        writes.visit_expr(test);
        proven.retain(|path| {
            !writes
                .paths
                .iter()
                .any(|write| receiver_is_within(path, write))
        });
        proven
    }

    fn future_test_proofs(&self, test: &Expr, truth: bool) -> HashSet<String> {
        match test {
            Expr::UnaryOp(u) if matches!(u.op, ast::UnaryOp::Not) => {
                self.future_test_proofs(&u.operand, !truth)
            }
            Expr::BoolOp(b) => {
                let all = matches!(
                    (b.op, truth),
                    (ast::BoolOp::And, true) | (ast::BoolOp::Or, false)
                );
                let mut proofs = b.values.iter().map(|v| self.future_test_proofs(v, truth));
                let mut proven = proofs.next().unwrap_or_default();
                for next in proofs {
                    if all {
                        proven.extend(next);
                    } else {
                        proven.retain(|path| next.contains(path));
                    }
                }
                proven
            }
            Expr::Call(c) if truth => {
                if let Expr::Attribute(attr) = c.func.as_ref()
                    && attr.attr.as_str() == "done"
                    && c.arguments.args.is_empty()
                    && c.arguments.keywords.is_empty()
                    && let Some(path) = expr_attr_path(&attr.value)
                    && self.is_concurrent_future_method(attr)
                {
                    return HashSet::from([path]);
                }
                HashSet::new()
            }
            _ => HashSet::new(),
        }
    }

    pub(crate) fn call_is_nonblocking_future_wait(&self, call: &ast::ExprCall) -> bool {
        let Expr::Attribute(attr) = call.func.as_ref() else {
            return false;
        };
        if !matches!(attr.attr.as_str(), "result" | "exception") {
            return false;
        }
        let bound = self.expr_is_external_bound_call(&call.func);
        let receiver = if bound {
            Some(attr.value.as_ref())
        } else {
            call.arguments.find_argument_value("self", 0)
        };
        let complete = receiver
            .and_then(expr_attr_path)
            .is_some_and(|path| self.future_done_receivers.contains(&path));
        let timeout = call
            .arguments
            .find_argument_value("timeout", usize::from(!bound));
        (complete || timeout.is_some_and(timeout_literal_is_nonpositive))
            && self.is_concurrent_future_method(attr)
    }

    pub(super) fn is_concurrent_future_method(&self, attr: &ast::ExprAttribute) -> bool {
        let definitions = definitions_for_attribute(self.model, attr);
        !definitions.is_empty()
            && definitions.iter().all(|r| {
                r.definition()
                    .and_then(|d| qualname_from_definition(self.model, d))
                    .and_then(|q| q.rsplit_once('.').map(|(class, _)| class.to_string()))
                    .is_some_and(|class| {
                        matches!(
                            class.as_str(),
                            "concurrent.futures.Future" | "concurrent.futures._base.Future"
                        )
                    })
            })
    }

    pub(super) fn invalidate_future_done_receiver(&mut self, path: &str) {
        self.future_done_receivers
            .retain(|receiver| !receiver_is_within(receiver, path));
    }

    /// A later loop iteration or exceptional path may observe any explicit
    /// write in this suite, but never a deferred function body's local writes.
    pub(super) fn invalidate_future_writes_in_body(&mut self, body: &[Stmt]) {
        if self.future_done_receivers.is_empty() {
            return;
        }
        let mut writes = ReceiverWrites::default();
        writes.visit_body(body);
        for path in writes.paths {
            self.invalidate_future_done_receiver(&path);
        }
    }

    pub(super) fn invalidate_future_writes_in_expr(&mut self, target: &Expr) {
        let mut writes = ReceiverWrites::default();
        writes.visit_expr(target);
        for path in writes.paths {
            self.invalidate_future_done_receiver(&path);
        }
    }

    pub(super) fn invalidate_future_writes_in_pattern(&mut self, pattern: &Pattern) {
        let mut writes = ReceiverWrites::default();
        writes.visit_pattern(pattern);
        for path in writes.paths {
            self.invalidate_future_done_receiver(&path);
        }
    }
}

fn timeout_literal_is_nonpositive(timeout: &Expr) -> bool {
    let (literal, negative) = match timeout {
        Expr::UnaryOp(u) if matches!(u.op, ast::UnaryOp::UAdd | ast::UnaryOp::USub) => {
            (u.operand.as_ref(), matches!(u.op, ast::UnaryOp::USub))
        }
        _ => (timeout, false),
    };
    // Numeric tokens are unsigned; the unary expression supplies their sign.
    // Do not share this with lock timeouts: Lock.acquire(timeout=-1) waits.
    match literal {
        Expr::NumberLiteral(n) => match &n.value {
            ast::Number::Int(i) => negative || i.as_u64() == Some(0),
            ast::Number::Float(f) => (if negative { -*f } else { *f }) <= 0.0,
            _ => false,
        },
        Expr::BooleanLiteral(b) => negative || !b.value,
        _ => false,
    }
}

fn receiver_is_within(receiver: &str, path: &str) -> bool {
    receiver == path
        || receiver
            .strip_prefix(path)
            .is_some_and(|tail| tail.starts_with('.'))
}

#[derive(Default)]
struct ReceiverWrites {
    paths: HashSet<String>,
}

impl<'a> Visitor<'a> for ReceiverWrites {
    fn visit_comprehension(&mut self, comp: &'a ast::Comprehension) {
        // Iteration targets belong to the comprehension's own scope. Walrus
        // assignments in the iter/filters/body still write to the outer scope.
        self.visit_expr(&comp.iter);
        for test in &comp.ifs {
            self.visit_expr(test);
        }
    }

    fn visit_stmt(&mut self, stmt: &'a Stmt) {
        match stmt {
            Stmt::FunctionDef(f) => {
                self.paths.insert(f.name.as_str().to_string());
            }
            Stmt::ClassDef(c) => {
                self.paths.insert(c.name.as_str().to_string());
            }
            Stmt::Import(i) => {
                for alias in &i.names {
                    let name = alias.asname.as_ref().map_or_else(
                        || alias.name.as_str().split('.').next().unwrap_or_default(),
                        |name| name.as_str(),
                    );
                    self.paths.insert(name.to_string());
                }
            }
            Stmt::ImportFrom(i) => {
                for alias in &i.names {
                    let name = alias.asname.as_ref().unwrap_or(&alias.name);
                    self.paths.insert(name.as_str().to_string());
                }
            }
            Stmt::AnnAssign(a) if a.value.is_none() => {}
            _ => walk_stmt(self, stmt),
        }
    }

    fn visit_pattern(&mut self, pattern: &'a Pattern) {
        let name = match pattern {
            Pattern::MatchAs(p) => p.name.as_ref(),
            Pattern::MatchStar(p) => p.name.as_ref(),
            Pattern::MatchMapping(p) => p.rest.as_ref(),
            _ => None,
        };
        if let Some(name) = name {
            self.paths.insert(name.as_str().to_string());
        }
        walk_pattern(self, pattern);
    }

    fn visit_expr(&mut self, expr: &'a Expr) {
        let context = match expr {
            Expr::Name(n) => Some(n.ctx),
            Expr::Attribute(a) => Some(a.ctx),
            // A lambda's body is deferred; only its defaults execute now.
            Expr::Lambda(l) => {
                if let Some(parameters) = &l.parameters {
                    self.visit_parameters(parameters);
                }
                return;
            }
            _ => None,
        };
        if context.is_some_and(|ctx| !matches!(ctx, ast::ExprContext::Load))
            && let Some(path) = expr_attr_path(expr)
        {
            self.paths.insert(path);
        }
        walk_expr(self, expr);
    }
}
