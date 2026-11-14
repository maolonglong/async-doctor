//! Try/match/for/with statement walkers for `CallCollector`.

use async_doctor_core::{CallKind, CalleeRef};
use ruff_python_ast::visitor::Visitor;
use ruff_python_ast::{self as ast, Expr};
use ruff_text_size::Ranged;

use super::super::loop_guards::except_is_runtime_error;
use super::CallCollector;
use crate::extract::constants::body_always_exits;

impl<'a, 'db> CallCollector<'a, 'db> {
    pub(crate) fn visit_try(&mut self, try_stmt: &'a ast::StmtTry) {
        self.maybe_record_swallowed_cancelled_error(try_stmt);
        let before = self.future_done_receivers.clone();
        let mut continuations = Vec::new();
        // Any try/except/else/finally path is guarded — not unconditional.
        let prev_saw = self.try_saw_get_loop;
        self.try_saw_get_loop = false;
        self.branch_depth += 1;
        self.visit_body_seq(&try_stmt.body);
        let normal = self.future_done_receivers.clone();
        let saw_get_loop = self.try_saw_get_loop;
        self.branch_depth -= 1;
        for handler in &try_stmt.handlers {
            self.future_done_receivers.clone_from(&before);
            self.invalidate_future_writes_in_body(&try_stmt.body);
            let ast::ExceptHandler::ExceptHandler(h) = handler;
            if let Some(type_) = &h.type_ {
                self.visit_expr(type_);
            }
            if let Some(name) = &h.name {
                self.invalidate_future_done_receiver(name.as_str());
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
            if !body_always_exits(&h.body) {
                continuations.push(self.future_done_receivers.clone());
            }
        }
        self.try_saw_get_loop = prev_saw;
        self.future_done_receivers = normal;
        self.branch_depth += 1;
        self.visit_body_seq(&try_stmt.orelse);
        self.branch_depth -= 1;
        if !body_always_exits(&try_stmt.body) && !body_always_exits(&try_stmt.orelse) {
            continuations.push(self.future_done_receivers.clone());
        }
        self.future_done_receivers = continuations.pop().unwrap_or_default();
        for branch in continuations {
            self.future_done_receivers
                .retain(|path| branch.contains(path));
        }
        // finally always runs, but keep conservative (still a try-related path).
        if !try_stmt.finalbody.is_empty() {
            // It also runs before an exception is caught or a return completes.
            // A proof established only on normal completion is insufficient.
            self.future_done_receivers = before;
            self.invalidate_future_writes_in_body(&try_stmt.body);
            for handler in &try_stmt.handlers {
                let ast::ExceptHandler::ExceptHandler(h) = handler;
                self.invalidate_future_writes_in_body(&h.body);
            }
            self.invalidate_future_writes_in_body(&try_stmt.orelse);
        }
        self.branch_depth += 1;
        self.visit_body_seq(&try_stmt.finalbody);
        self.branch_depth -= 1;
    }

    pub(crate) fn visit_match(&mut self, match_stmt: &'a ast::StmtMatch) {
        self.visit_expr(&match_stmt.subject);
        let mut remaining = self.future_done_receivers.clone();
        let mut after = remaining.clone();
        for case in &match_stmt.cases {
            self.future_done_receivers.clone_from(&remaining);
            self.invalidate_future_writes_in_pattern(&case.pattern);
            if let Some(guard) = &case.guard {
                self.visit_expr(guard);
                // Pattern/guard writes persist even if this guard rejects it.
                remaining.retain(|path| self.future_done_receivers.contains(path));
                after.retain(|path| self.future_done_receivers.contains(path));
                self.future_done_receivers
                    .extend(self.futures_proven_done_in_test(guard, true));
            }
            self.branch_depth += 1;
            self.visit_body_seq(&case.body);
            self.branch_depth -= 1;
            if !body_always_exits(&case.body) {
                after.retain(|path| self.future_done_receivers.contains(path));
            }
        }
        self.future_done_receivers = after;
    }

    pub(crate) fn visit_for(&mut self, for_stmt: &'a ast::StmtFor) {
        // Record __iter__/__aiter__ edge when cheap to resolve.
        if let Some(caller) = self.current_fn
            && self.dead_depth == 0
        {
            let invalid = self.maybe_record_protocol_misuse(
                caller,
                &for_stmt.iter,
                CallKind::ForIter,
                for_stmt.is_async,
            );
            if !invalid
                && !for_stmt.is_async
                && let Some(callee) = self.resolve_dunder(&for_stmt.iter, "__iter__")
            {
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
        // `async for x in call(...)`: the iter Call is an async use of that one call.
        if !for_stmt.is_async {
            self.consume_named_generator(&for_stmt.iter, false);
        }
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
        let before = self.future_done_receivers.clone();
        self.invalidate_future_writes_in_expr(&for_stmt.target);
        self.invalidate_future_writes_in_body(&for_stmt.body);
        self.loop_depth += 1;
        self.visit_body_seq(&for_stmt.body);
        self.loop_depth -= 1;
        self.future_done_receivers
            .retain(|path| before.contains(path));
        let before_else = self.future_done_receivers.clone();
        self.visit_body_seq(&for_stmt.orelse);
        self.future_done_receivers
            .retain(|path| before_else.contains(path));
    }

    pub(crate) fn visit_with(&mut self, with_stmt: &'a ast::StmtWith) {
        if let Some(caller) = self.current_fn
            && self.dead_depth == 0
            && !with_stmt.is_async
        {
            for item in &with_stmt.items {
                if self.maybe_record_protocol_misuse(
                    caller,
                    &item.context_expr,
                    CallKind::WithEnter,
                    false,
                ) {
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
                }
                if let Some(callee) = self.resolve_dunder(&item.context_expr, "__enter__")
                    && (!matches!(item.context_expr, Expr::Call(_))
                        || matches!(callee, CalleeRef::Project(_)))
                {
                    self.clear_pending_call_args();
                    self.pending_is_bound_call = matches!(callee, CalleeRef::Project(_));
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
                    self.clear_pending_call_args();
                    self.pending_is_bound_call = matches!(callee, CalleeRef::Project(_));
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
                    if self.maybe_record_protocol_misuse(
                        caller,
                        &item.context_expr,
                        CallKind::WithEnter,
                        true,
                    ) {
                        if let Expr::Call(call) = &item.context_expr {
                            self.walk_call_children(call, None);
                        }
                        continue;
                    }
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
                if let Some(caller) = self.current_fn {
                    for (dunder, kind) in [
                        ("__aenter__", CallKind::WithEnter),
                        ("__aexit__", CallKind::WithExit),
                    ] {
                        if let Some(callee @ CalleeRef::Project(_)) =
                            self.resolve_dunder(&item.context_expr, dunder)
                        {
                            self.clear_pending_call_args();
                            self.pending_is_bound_call = true;
                            self.pending_is_awaited = true;
                            self.push_edge(
                                caller,
                                callee,
                                item.context_expr.range(),
                                kind,
                                false,
                                None,
                            );
                        }
                    }
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
        let before = self.future_done_receivers.clone();
        self.visit_body_seq(&with_stmt.body);
        let normal = std::mem::replace(&mut self.future_done_receivers, before);
        // __exit__/__aexit__ may suppress an exception before a body-only proof.
        self.invalidate_future_writes_in_body(&with_stmt.body);
        self.future_done_receivers
            .retain(|path| normal.contains(path));
    }

    pub(crate) fn visit_comprehension_like(
        &mut self,
        elt: &'a Expr,
        key: Option<&'a Expr>,
        generators: &'a [ast::Comprehension],
    ) {
        let before = self.future_done_receivers.clone();
        let has_filter = generators.iter().any(|g| !g.ifs.is_empty());
        if has_filter {
            self.branch_depth += 1;
        }
        for comp in generators {
            if let Some(caller) = self.current_fn {
                self.maybe_record_protocol_misuse(
                    caller,
                    &comp.iter,
                    CallKind::GeneratorConsume,
                    comp.is_async,
                );
            }
            if !comp.is_async {
                self.consume_named_generator(&comp.iter, false);
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
        // Comprehension targets do not rebind outer locals; named expressions
        // do. Preserve that distinction when leaving the synthetic scope.
        self.future_done_receivers = before;
        for comp in generators {
            self.invalidate_future_writes_in_expr(&comp.iter);
            for test in &comp.ifs {
                self.invalidate_future_writes_in_expr(test);
            }
        }
        if let Some(k) = key {
            self.invalidate_future_writes_in_expr(k);
        }
        self.invalidate_future_writes_in_expr(elt);
    }
}
