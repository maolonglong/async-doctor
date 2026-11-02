//! Module-level constants, param/cache guards, and early-exit helpers.

use std::collections::{HashMap, HashSet};

use async_doctor_core::{LiteralArg, ParamGuard};
use ruff_python_ast::visitor::{Visitor, walk_expr, walk_stmt};
use ruff_python_ast::{self as ast, Expr, Stmt};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ModuleConst {
    True,
    False,
    None,
    /// Empty tuple/list/dict/set/string — falsy.
    Empty,
}

pub(crate) fn body_has_yield(body: &[Stmt]) -> bool {
    struct YieldFinder {
        found: bool,
    }
    impl<'a> Visitor<'a> for YieldFinder {
        fn visit_expr(&mut self, expr: &'a Expr) {
            if self.found {
                return;
            }
            match expr {
                Expr::Yield(_) | Expr::YieldFrom(_) => {
                    self.found = true;
                }
                _ => walk_expr(self, expr),
            }
        }
        fn visit_stmt(&mut self, stmt: &'a Stmt) {
            if self.found {
                return;
            }
            // Nested function defs have their own generator status.
            if matches!(stmt, Stmt::FunctionDef(_)) {
                return;
            }
            walk_stmt(self, stmt);
        }
    }
    let mut v = YieldFinder { found: false };
    for s in body {
        v.visit_stmt(s);
        if v.found {
            return true;
        }
    }
    false
}

pub(crate) fn const_value(expr: &Expr) -> Option<ModuleConst> {
    match expr {
        Expr::BooleanLiteral(b) => Some(if b.value {
            ModuleConst::True
        } else {
            ModuleConst::False
        }),
        Expr::NoneLiteral(_) => Some(ModuleConst::None),
        Expr::Tuple(t) if t.elts.is_empty() => Some(ModuleConst::Empty),
        Expr::List(l) if l.elts.is_empty() => Some(ModuleConst::Empty),
        Expr::Dict(d) if d.items.is_empty() => Some(ModuleConst::Empty),
        Expr::Set(s) if s.elts.is_empty() => Some(ModuleConst::Empty),
        Expr::StringLiteral(s) if s.value.to_str().is_empty() => Some(ModuleConst::Empty),
        Expr::Call(call)
            if matches!(
                call.func.as_ref(),
                Expr::Name(n) if matches!(n.id.as_str(), "dict" | "list" | "set" | "tuple")
            ) && call.arguments.args.is_empty()
                && call.arguments.keywords.is_empty() =>
        {
            Some(ModuleConst::Empty)
        }
        _ => None,
    }
}

/// Names that appear in any `global NAME` in this module — config toggles; never prune.
pub(crate) fn collect_names_declared_global(suite: &[Stmt]) -> HashSet<String> {
    let mut out = HashSet::new();
    fn walk(stmts: &[Stmt], out: &mut HashSet<String>) {
        for stmt in stmts {
            match stmt {
                Stmt::Global(g) => {
                    for n in &g.names {
                        out.insert(n.as_str().to_string());
                    }
                }
                Stmt::FunctionDef(f) => walk(&f.body, out),
                Stmt::ClassDef(c) => walk(&c.body, out),
                Stmt::If(i) => {
                    walk(&i.body, out);
                    for c in &i.elif_else_clauses {
                        walk(&c.body, out);
                    }
                }
                Stmt::For(f) => {
                    walk(&f.body, out);
                    walk(&f.orelse, out);
                }
                Stmt::While(w) => {
                    walk(&w.body, out);
                    walk(&w.orelse, out);
                }
                Stmt::With(w) => walk(&w.body, out),
                Stmt::Try(t) => {
                    walk(&t.body, out);
                    for h in &t.handlers {
                        let ast::ExceptHandler::ExceptHandler(eh) = h;
                        walk(&eh.body, out);
                    }
                    walk(&t.orelse, out);
                    walk(&t.finalbody, out);
                }
                Stmt::Match(m) => {
                    for c in &m.cases {
                        walk(&c.body, out);
                    }
                }
                _ => {}
            }
        }
    }
    walk(suite, &mut out);
    out
}

pub(crate) fn collect_module_constants(suite: &[Stmt]) -> HashMap<String, ModuleConst> {
    let mut assign_count: HashMap<String, u32> = HashMap::new();
    let mut values: HashMap<String, ModuleConst> = HashMap::new();
    let globals = collect_names_declared_global(suite);

    let bump = |counts: &mut HashMap<String, u32>, name: &str| {
        *counts.entry(name.to_string()).or_insert(0) += 1;
    };

    for stmt in suite {
        match stmt {
            Stmt::Assign(a) => {
                if a.targets.len() != 1 {
                    continue;
                }
                let Expr::Name(n) = &a.targets[0] else {
                    continue;
                };
                let name = n.id.as_str();
                bump(&mut assign_count, name);
                if let Some(v) = const_value(&a.value) {
                    values.insert(name.to_string(), v);
                } else {
                    values.remove(name);
                }
            }
            Stmt::AnnAssign(a) => {
                let Expr::Name(n) = a.target.as_ref() else {
                    continue;
                };
                let name = n.id.as_str();
                bump(&mut assign_count, name);
                if let Some(val) = &a.value {
                    if let Some(v) = const_value(val) {
                        values.insert(name.to_string(), v);
                    } else {
                        values.remove(name);
                    }
                }
            }
            Stmt::AugAssign(a) => {
                if let Expr::Name(n) = a.target.as_ref() {
                    bump(&mut assign_count, n.id.as_str());
                    // Force count >= 2 so we drop it as unstable.
                    bump(&mut assign_count, n.id.as_str());
                    values.remove(n.id.as_str());
                }
            }
            _ => {}
        }
    }

    values.retain(|k, _| assign_count.get(k).copied().unwrap_or(0) == 1 && !globals.contains(k));
    values
}

/// Names of keyword arguments passed a literal `True` (`exist_ok=True`).
pub(crate) fn keyword_true_names(keywords: &[ruff_python_ast::Keyword]) -> Vec<String> {
    keywords
        .iter()
        .filter(|k| matches!(&k.value, Expr::BooleanLiteral(b) if b.value))
        .filter_map(|k| k.arg.as_ref().map(|a| a.as_str().to_string()))
        .collect()
}

pub(crate) fn literal_arg(expr: &Expr) -> Option<LiteralArg> {
    match expr {
        Expr::BooleanLiteral(b) => Some(if b.value {
            LiteralArg::True
        } else {
            LiteralArg::False
        }),
        Expr::NoneLiteral(_) => Some(LiteralArg::None),
        _ => None,
    }
}

pub(crate) fn is_generator_consumer(func: &Expr) -> bool {
    match func {
        Expr::Name(n) => matches!(
            n.id.as_str(),
            "list" | "tuple" | "set" | "next" | "sorted" | "frozenset"
        ),
        Expr::Attribute(a) => matches!(
            a.attr.as_str(),
            "list" | "tuple" | "set" | "next" | "sorted" | "frozenset"
        ),
        _ => false,
    }
}

/// Detect `if flag:` / `if not flag:` where `flag` is a positional parameter.
pub(crate) fn param_guard_from_test(test: &Expr, param_names: &[String]) -> Option<ParamGuard> {
    let (name, inverted) = match test {
        Expr::Name(n) => (n.id.as_str(), false),
        Expr::UnaryOp(u) if matches!(u.op, ast::UnaryOp::Not) => match u.operand.as_ref() {
            Expr::Name(n) => (n.id.as_str(), true),
            _ => return None,
        },
        _ => return None,
    };
    param_names
        .iter()
        .position(|p| p == name)
        .map(|index| ParamGuard {
            index: index as u32,
            inverted,
        })
}

/// Cheap cache-miss tests: `x is None`, `k not in cache`.
pub(crate) fn is_cache_miss_test(test: &Expr) -> bool {
    match test {
        Expr::Compare(cmp) if cmp.ops.len() == 1 => {
            let op = cmp.ops[0];
            match op {
                ast::CmpOp::Is => {
                    // `cache is None`
                    matches!(cmp.operands.get(1), Some(Expr::NoneLiteral(_)))
                        && matches!(cmp.operands.first(), Some(Expr::Name(_)))
                }
                ast::CmpOp::NotIn => {
                    // `key not in cache`
                    matches!(cmp.operands.get(1), Some(Expr::Name(_)))
                }
                _ => false,
            }
        }
        _ => false,
    }
}

/// Cache-*hit* early-exit tests: `x is not None`, `k in cache`.
pub(crate) fn is_cache_hit_test(test: &Expr) -> bool {
    match test {
        Expr::Compare(cmp) if cmp.ops.len() == 1 => match cmp.ops[0] {
            ast::CmpOp::IsNot => {
                matches!(cmp.operands.get(1), Some(Expr::NoneLiteral(_)))
                    && matches!(cmp.operands.first(), Some(Expr::Name(_)))
            }
            ast::CmpOp::In => matches!(cmp.operands.get(1), Some(Expr::Name(_))),
            _ => false,
        },
        _ => false,
    }
}

pub(crate) fn body_always_exits(body: &[Stmt]) -> bool {
    !body.is_empty()
        && body
            .iter()
            .all(|s| matches!(s, Stmt::Return(_) | Stmt::Raise(_)))
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum EarlyExitKind {
    /// Guard is provably always true and body exits — rest of suite is dead.
    UnreachableRest,
    /// Guard may fail — rest of suite is conditional (optionally cache-miss /
    /// proven no-running-loop after `if loop is not None: return`).
    Conditional {
        cache_miss: bool,
        /// Rest of the suite only runs when there is no running event loop
        /// (early-return after `loop is not None` / `loop.is_running()` / …).
        no_running_loop: bool,
    },
}
