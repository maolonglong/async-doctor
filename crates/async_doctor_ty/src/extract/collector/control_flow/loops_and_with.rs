//! Try/match/for/with statement walkers for `CallCollector`.

use async_doctor_core::CallKind;
use ruff_python_ast::visitor::Visitor;
use ruff_python_ast::{self as ast, Expr};
use ruff_text_size::Ranged;

use super::super::loop_guards::except_is_runtime_error;
use super::CallCollector;

impl<'a, 'db> CallCollector<'a, 'db> {
    pub(crate) fn visit_try(&mut self, try_stmt: &'a ast::StmtTry) {
        self.maybe_record_swallowed_cancelled_error(try_stmt);
        // Any try/except/else/finally path is guarded — not unconditional.
        let prev_saw = self.try_saw_get_loop;
        self.try_saw_get_loop = false;
        self.branch_depth += 1;
        self.visit_body_seq(&try_stmt.body);
        let saw_get_loop = self.try_saw_get_loop;
        self.branch_depth -= 1;
        for handler in &try_stmt.handlers {
            let ast::ExceptHandler::ExceptHandler(h) = handler;
            if let Some(type_) = &h.type_ {
                self.visit_expr(type_);
            }
            let runtime_error = saw_get_loop && except_is_runtime_error(h.type_.as_deref());
            self.branch_depth += 1;
            if runtime_error {
                self.no_running_loop_depth += 1;
            }
            self.visit_body_seq(&h.body);
            if runtime_error {
                self.no_running_loop_depth -= 1;
            }
            self.branch_depth -= 1;
        }
        self.try_saw_get_loop = prev_saw;
        self.branch_depth += 1;
        self.visit_body_seq(&try_stmt.orelse);
        self.branch_depth -= 1;
        // finally always runs, but keep conservative (still a try-related path).
        self.branch_depth += 1;
        self.visit_body_seq(&try_stmt.finalbody);
        self.branch_depth -= 1;
    }

    pub(crate) fn visit_match(&mut self, match_stmt: &'a ast::StmtMatch) {
        self.visit_expr(&match_stmt.subject);
        for case in &match_stmt.cases {
            self.branch_depth += 1;
            self.visit_body_seq(&case.body);
            self.branch_depth -= 1;
        }
    }

    pub(crate) fn visit_for(&mut self, for_stmt: &'a ast::StmtFor) {
        // Record __iter__/__aiter__ edge when cheap to resolve.
        if let Some(caller) = self.current_fn
            && self.dead_depth == 0
            && !for_stmt.is_async
        {
            if !self.in_lambda
                && self.off_loop_thread_depth == 0
                && !self.function(caller).is_installed_third_party()
                && self.is_sync_for_async_iterable(&for_stmt.iter)
            {
                self.push_sync_for_async_iterable_edge(caller, &for_stmt.iter, CallKind::ForIter);
            } else {
                let dunder = "__iter__";
                if let Some(callee) = self.resolve_dunder(&for_stmt.iter, dunder) {
                    self.push_edge(
                        caller,
                        callee,
                        for_stmt.iter.range(),
                        CallKind::ForIter,
                        false,
                        None,
                    );
                }
            }
        }
        // `async for x in call(...)`: the iter Call is an async use of that one call.
        self.consuming_generator = true;
        if for_stmt.is_async {
            if let Expr::Call(call) = for_stmt.iter.as_ref() {
                let is_offload = self.handle_call(call, true);
                self.walk_call_children(call, is_offload);
            } else {
                self.visit_expr(&for_stmt.iter);
            }
            self.note_async_for_receivers(for_stmt);
        } else {
            self.visit_expr(&for_stmt.iter);
        }
        self.consuming_generator = false;
        self.loop_depth += 1;
        self.visit_body_seq(&for_stmt.body);
        self.loop_depth -= 1;
        self.visit_body_seq(&for_stmt.orelse);
    }

    pub(crate) fn visit_with(&mut self, with_stmt: &'a ast::StmtWith) {
        if let Some(caller) = self.current_fn
            && self.dead_depth == 0
            && !with_stmt.is_async
        {
            for item in &with_stmt.items {
                if self.is_sync_with_async_cm(&item.context_expr) {
                    self.push_sync_with_async_cm_edge(caller, &item.context_expr);
                    if let Expr::Call(call) = &item.context_expr {
                        self.walk_call_children(call, None);
                    }
                    continue;
                }
                // `with engine.begin() as conn:` — the Call itself is the enter site
                // for with_only sinks (Engine.begin). Dunder __enter__ on the *return*
                // type is secondary and often unnamed for stubs.
                if let Expr::Call(call) = &item.context_expr {
                    self.pending_with_enter = true;
                    let is_offload = self.handle_call(call, false);
                    self.pending_with_enter = false;
                    self.walk_call_children(call, is_offload);
                } else if let Some(callee) = self.resolve_dunder(&item.context_expr, "__enter__") {
                    self.push_edge(
                        caller,
                        callee,
                        item.context_expr.range(),
                        CallKind::WithEnter,
                        false,
                        None,
                    );
                }
                if let Some(callee) = self.resolve_dunder(&item.context_expr, "__exit__") {
                    self.push_edge(
                        caller,
                        callee,
                        item.context_expr.range(),
                        CallKind::WithExit,
                        false,
                        None,
                    );
                }
            }
        }
        for item in &with_stmt.items {
            // `async with call(...) as x`: mark that one call awaited; bind asyncio receivers.
            if with_stmt.is_async {
                if let Some(caller) = self.current_fn {
                    self.maybe_record_async_with_ephemeral_client(caller, item);
                    self.maybe_record_useless_local_async_lock(caller, item);
                }
                if let Expr::Call(call) = &item.context_expr {
                    self.pending_with_enter = true;
                    let is_offload = self.handle_call(call, true);
                    self.pending_with_enter = false;
                    self.walk_call_children(call, is_offload);
                } else {
                    self.visit_expr(&item.context_expr);
                }
                self.note_async_with_receiver(item);
                self.note_async_with_stateful_session(item);
            } else if !matches!(item.context_expr, Expr::Call(_)) {
                // Sync `with Call(...)` already recorded above as WithEnter.
                self.visit_expr(&item.context_expr);
            }
            // Bind `with ZipFile(buf) as zf` / buffer receivers for later method skips.
            if let Some(Expr::Name(n)) = item.optional_vars.as_deref() {
                self.note_local_binding(n.id.as_str(), &item.context_expr);
                if with_stmt.is_async {
                    // `async with` ephemeral clients are already recorded above;
                    // re-apply async-with receiver/session bindings after `note_local_binding`.
                    self.local_ephemeral_clients.remove(n.id.as_str());
                    self.note_async_with_receiver(item);
                    self.note_async_with_stateful_session(item);
                }
            }
            if let Some(opt) = &item.optional_vars {
                self.visit_expr(opt);
            }
        }
        if let Some(caller) = self.current_fn {
            self.maybe_record_taskgroup_shared_session(caller, with_stmt);
        }
        self.visit_body_seq(&with_stmt.body);
    }

    pub(crate) fn visit_comprehension_like(
        &mut self,
        elt: &'a Expr,
        key: Option<&'a Expr>,
        generators: &'a [ast::Comprehension],
    ) {
        let has_filter = generators.iter().any(|g| !g.ifs.is_empty());
        if has_filter {
            self.branch_depth += 1;
        }
        for comp in generators {
            if !comp.is_async
                && self.dead_depth == 0
                && !self.in_lambda
                && self.off_loop_thread_depth == 0
                && let Some(caller) = self.current_fn
                && !self.function(caller).is_installed_third_party()
                && self.is_sync_for_async_iterable(&comp.iter)
            {
                self.push_sync_for_async_iterable_edge(
                    caller,
                    &comp.iter,
                    CallKind::GeneratorConsume,
                );
            }
            self.consuming_generator = true;
            if comp.is_async {
                if let Expr::Call(call) = &comp.iter {
                    let is_offload = self.handle_call(call, true);
                    self.walk_call_children(call, is_offload);
                } else {
                    self.visit_expr(&comp.iter);
                }
            } else {
                self.visit_expr(&comp.iter);
            }
            self.consuming_generator = false;
            self.visit_expr(&comp.target);
            for test in &comp.ifs {
                self.visit_expr(test);
            }
        }
        if let Some(k) = key {
            self.visit_expr(k);
        }
        self.visit_expr(elt);
        if has_filter {
            self.branch_depth -= 1;
        }
    }
}
