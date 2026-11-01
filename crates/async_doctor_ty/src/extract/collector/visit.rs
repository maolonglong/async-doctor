//! AST visitor entry points for [`super::CallCollector`].

use ruff_python_ast::visitor::{Visitor, walk_expr, walk_stmt};
use ruff_python_ast::{self as ast, Expr, Stmt};

use super::CallCollector;
use super::loop_guards::collect_idle_loop_helpers;
use super::thread_guards::futures_proven_done_in_test;
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
                let prev_running_loop = std::mem::take(&mut self.running_loop_locals);
                let prev_event_loop = std::mem::take(&mut self.event_loop_locals);
                // Module-level unbounded queues remain visible inside functions;
                // clone so local rebinds do not leak to sibling defs.
                let prev_queues = self.unbounded_queues.clone();
                let prev_boto = self.boto_receivers.clone();
                let prev_boto_session = self.boto_session_receivers.clone();
                let prev_aio_boto = self.aio_boto_receivers.clone();
                let prev_buffers = self.buffer_locals.clone();
                let prev_archives = self.in_memory_archive_receivers.clone();
                self.current_fn = fid;
                self.current_param_names = fid
                    .map(|id| self.function(id).param_names.clone())
                    .unwrap_or_default();
                // Parameters shadow module-level unbounded queue / boto bindings.
                for p in &self.current_param_names {
                    self.unbounded_queues.remove(p);
                    self.boto_receivers.remove(p);
                    self.boto_session_receivers.remove(p);
                    self.aio_boto_receivers.remove(p);
                    self.buffer_locals.remove(p);
                    self.in_memory_archive_receivers.remove(p);
                }
                self.fn_stack.push(func.name.as_str().to_string());
                self.visit_body_seq(&func.body);
                self.fn_stack.pop();
                self.current_fn = prev;
                self.current_param_names = prev_params;
                self.local_lambdas = prev_lambdas;
                self.local_genexprs = prev_genexprs;
                self.idle_loop_locals = prev_idle_locals;
                self.running_loop_locals = prev_running_loop;
                self.event_loop_locals = prev_event_loop;
                self.unbounded_queues = prev_queues;
                self.boto_receivers = prev_boto;
                self.boto_session_receivers = prev_boto_session;
                self.aio_boto_receivers = prev_aio_boto;
                self.buffer_locals = prev_buffers;
                self.in_memory_archive_receivers = prev_archives;
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
            // `fut.done() and fut.result()` / `self.x.done() and self.x.result()`:
            // accumulate done-proven receivers across And conjuncts left→right.
            Expr::BoolOp(b) if matches!(b.op, ast::BoolOp::And) => {
                let prev = self.future_done_receivers.clone();
                for v in &b.values {
                    self.visit_expr(v);
                    for path in futures_proven_done_in_test(v) {
                        self.future_done_receivers.insert(path);
                    }
                }
                self.future_done_receivers = prev;
            }
            // `X.result() if X.done() else None`
            Expr::If(ifexp) => {
                self.visit_expr(&ifexp.test);
                let proven = futures_proven_done_in_test(&ifexp.test);
                let prev = if !proven.is_empty() {
                    let prev = self.future_done_receivers.clone();
                    for path in &proven {
                        self.future_done_receivers.insert(path.clone());
                    }
                    Some(prev)
                } else {
                    None
                };
                self.visit_expr(&ifexp.body);
                if let Some(prev) = prev {
                    self.future_done_receivers = prev;
                }
                self.visit_expr(&ifexp.orelse);
            }
            Expr::Attribute(attr) => {
                if !self.in_callee_position {
                    self.maybe_property_get(attr);
                }
                walk_expr(self, expr);
            }
            Expr::Name(name) => {
                // Consuming a locally-assigned genexpr by name.
                if self.consuming_generator {
                    self.expand_named_genexpr(name.id.as_str());
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
                    self.expand_local_genexpr(g);
                }
            }
            Expr::DictComp(dc) => {
                self.visit_comprehension_like(&dc.value, dc.key.as_deref(), &dc.generators)
            }
            _ => walk_expr(self, expr),
        }
    }
}

impl<'a, 'db> CallCollector<'a, 'db> {
    /// Expand the newest non-cyclic genexpr bound to `name`.
    ///
    /// On `out = (sleep…); out = (x for x in out)`, expanding the second
    /// genexpr visits `out` while the second is already in
    /// [`Self::expanding_genexprs`] — resolve to the previous binding instead.
    fn expand_named_genexpr(&mut self, name: &str) {
        let Some(stack) = self.local_genexprs.get(name) else {
            return;
        };
        // Newest → oldest; skip bindings already on the expand stack.
        let chosen = stack
            .iter()
            .rev()
            .copied()
            .find(|g| !self.expanding_genexprs.contains(&std::ptr::from_ref(*g)));
        if let Some(gexpr) = chosen {
            self.expand_local_genexpr(gexpr);
        }
    }

    /// Expand a genexpr under a consumer, guarding against self-referential
    /// reassignment cycles (`out = (x for x in out)`) and exponential fan-out
    /// (`g = (a for a in prev for b in prev)`).
    fn expand_local_genexpr(&mut self, gexpr: &'a ruff_python_ast::ExprGenerator) {
        if self.genexpr_expand_depth >= super::MAX_GENEXPR_EXPAND_DEPTH {
            return;
        }
        let outermost = self.genexpr_expand_depth == 0;
        if outermost {
            // Fresh memo/budget per top-level consume so a later `list(g)` (e.g.
            // in an else branch) still expands after an earlier offloaded consume.
            self.expanded_genexpr_memo.clear();
            self.genexpr_expand_visits = 0;
        }
        if self.genexpr_expand_visits >= super::MAX_GENEXPR_EXPAND_VISITS {
            return;
        }
        let ptr = std::ptr::from_ref(gexpr);
        // Already fully expanded under this consume — skip duplicate fan-out arms.
        if self.expanded_genexpr_memo.contains(&ptr) {
            return;
        }
        if !self.expanding_genexprs.insert(ptr) {
            return;
        }
        self.expanded_genexpr_memo.insert(ptr);
        self.genexpr_expand_visits += 1;
        self.genexpr_expand_depth += 1;
        self.visit_comprehension_like(&gexpr.elt, None, &gexpr.generators);
        self.genexpr_expand_depth -= 1;
        self.expanding_genexprs.remove(&ptr);
    }
}
