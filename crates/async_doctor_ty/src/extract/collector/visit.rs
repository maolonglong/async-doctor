//! AST visitor entry points for [`super::CallCollector`].

use ruff_python_ast::visitor::{Visitor, walk_expr, walk_stmt};
use ruff_python_ast::{Expr, Stmt};

use super::CallCollector;
use super::loop_guards::collect_idle_loop_helpers;
use crate::extract::ctx::scope_qualname;

impl<'a, 'db> Visitor<'a> for CallCollector<'a, 'db> {
    fn visit_stmt(&mut self, stmt: &'a Stmt) {
        if self.skip_lambda_body {
            return;
        }
        match stmt {
            Stmt::FunctionDef(func) => {
                let qualname =
                    scope_qualname(&self.class_stack, &self.fn_stack, func.name.as_str());
                let fid = self.qual_get(&(self.ctx.module.clone(), qualname));
                let prev = self.current_fn;
                let prev_params = std::mem::take(&mut self.current_param_names);
                let prev_lambdas = std::mem::take(&mut self.local_lambdas);
                let prev_genexprs = std::mem::take(&mut self.local_genexprs);
                let prev_idle_locals = std::mem::take(&mut self.idle_loop_locals);
                self.current_fn = fid;
                self.current_param_names = fid
                    .map(|id| self.facts.function(id).param_names.clone())
                    .unwrap_or_default();
                self.fn_stack.push(func.name.as_str().to_string());
                self.visit_body_seq(&func.body);
                self.fn_stack.pop();
                self.current_fn = prev;
                self.current_param_names = prev_params;
                self.local_lambdas = prev_lambdas;
                self.local_genexprs = prev_genexprs;
                self.idle_loop_locals = prev_idle_locals;
            }
            Stmt::ClassDef(class) => {
                // Pre-scan methods so later-defined idle helpers are visible to
                // earlier methods in the same class body.
                collect_idle_loop_helpers(&class.body, &mut self.idle_loop_helpers);
                self.class_stack.push(class.name.as_str().to_string());
                self.visit_body_seq(&class.body);
                self.class_stack.pop();
            }
            Stmt::Assign(assign) => {
                self.note_local_assign(assign);
                walk_stmt(self, stmt);
            }
            Stmt::AnnAssign(ann) => {
                if let (Expr::Name(n), Some(value)) = (ann.target.as_ref(), ann.value.as_ref()) {
                    self.note_local_binding(n.id.as_str(), value);
                }
                walk_stmt(self, stmt);
            }
            Stmt::If(if_stmt) => self.visit_if(if_stmt),
            Stmt::For(for_stmt) => {
                self.visit_for(for_stmt);
            }
            Stmt::While(while_stmt) => {
                self.loop_depth += 1;
                self.visit_expr(&while_stmt.test);
                self.visit_body_seq(&while_stmt.body);
                self.loop_depth -= 1;
                self.visit_body_seq(&while_stmt.orelse);
            }
            Stmt::With(with_stmt) => self.visit_with(with_stmt),
            Stmt::Try(try_stmt) => self.visit_try(try_stmt),
            Stmt::Match(match_stmt) => self.visit_match(match_stmt),
            Stmt::Import(_) | Stmt::ImportFrom(_) => {
                super::imports::note_if_import(self, stmt);
                walk_stmt(self, stmt);
            }
            other => walk_stmt(self, other),
        }
    }

    fn visit_expr(&mut self, expr: &'a Expr) {
        if self.skip_lambda_body {
            return;
        }
        match expr {
            Expr::Lambda(lambda) => {
                // Deferred: only walk parameters unless later called / offloaded specially.
                if let Some(parameters) = &lambda.parameters {
                    self.visit_parameters(parameters);
                }
                // Do not walk lambda body by default (deferred execution).
            }
            Expr::Await(await_expr) => {
                // Only the Call that is the *direct* await operand is marked awaited;
                // nested calls in arguments (e.g. await send(open(p).read())) stay sync sinks.
                if let Expr::Call(call) = await_expr.value.as_ref() {
                    self.handle_call(call, true);
                    self.walk_call_children(call);
                } else {
                    self.visit_expr(&await_expr.value);
                }
            }
            Expr::Call(call) => {
                self.handle_call(call, false);
                self.walk_call_children(call);
            }
            Expr::Attribute(attr) => {
                if !self.in_callee_position {
                    self.maybe_property_get(attr);
                }
                walk_expr(self, expr);
            }
            Expr::Name(name) => {
                // Consuming a locally-assigned genexpr by name.
                if self.consuming_generator
                    && let Some(gexpr) = self.local_genexprs.get(name.id.as_str()).copied()
                {
                    self.visit_comprehension_like(&gexpr.elt, None, &gexpr.generators);
                }
                walk_expr(self, expr);
            }
            Expr::YieldFrom(yf) => {
                self.consuming_generator = true;
                self.visit_expr(&yf.value);
                self.consuming_generator = false;
            }
            Expr::ListComp(lc) => self.visit_comprehension_like(&lc.elt, None, &lc.generators),
            Expr::SetComp(sc) => self.visit_comprehension_like(&sc.elt, None, &sc.generators),
            // Bare genexpr is deferred — only walk when consumed.
            Expr::Generator(g) => {
                if self.consuming_generator {
                    self.visit_comprehension_like(&g.elt, None, &g.generators);
                }
            }
            Expr::DictComp(dc) => {
                self.visit_comprehension_like(&dc.value, dc.key.as_deref(), &dc.generators)
            }
            _ => walk_expr(self, expr),
        }
    }
}
