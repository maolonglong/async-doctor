//! Control-flow-aware body walking for call collection.

use async_doctor_core::{CallKind, ConditionTag};
use ruff_db::files::FilePath;
use ruff_python_ast::visitor::Visitor;
use ruff_python_ast::{self as ast, Expr, Stmt};
use ruff_text_size::Ranged;
use ty_python_core::definition::DefinitionKind;
use ty_python_semantic::{ImportAliasResolution, definitions_for_name};

use super::CallCollector;
use super::loop_guards::except_is_runtime_error;
use super::thread_guards::{
    futures_proven_done_in_test, is_off_loop_thread_test, is_on_loop_thread_test,
};
use crate::extract::constants::{
    EarlyExitKind, ModuleConst, body_always_exits, is_cache_hit_test, is_cache_miss_test,
    param_guard_from_test,
};
use crate::extract::helpers::narrow_from_test;
use crate::extract::path::relative_display;

impl<'a, 'db> CallCollector<'a, 'db> {
    pub(crate) fn conditions_now(&self) -> Vec<ConditionTag> {
        let mut tags = Vec::new();
        if self.loop_depth > 0 {
            tags.push(ConditionTag::Loop);
        }
        // Prove-only: never emit unconditional/all-paths under any guard.
        if self.no_running_loop_depth > 0 {
            tags.push(ConditionTag::NoRunningLoopGuard);
        } else if self.cache_miss_depth > 0 {
            tags.push(ConditionTag::CacheMissDependent);
        } else if self.branch_depth > 0 {
            tags.push(ConditionTag::AsyncBranch);
        } else if self.loop_depth == 0 {
            tags.push(ConditionTag::Unconditional);
            tags.push(ConditionTag::AllPaths);
        }
        tags
    }

    /// Walk a statement suite, applying early-exit effects to subsequent stmts.
    pub(crate) fn visit_body_seq(&mut self, body: &'a [Stmt]) {
        let mut after_branch = 0u32;
        let mut after_cache = 0u32;
        let mut after_no_loop = 0u32;
        let mut after_dead = 0u32;
        for stmt in body {
            if let Stmt::If(if_stmt) = stmt {
                self.visit_if(if_stmt);
                if after_dead == 0
                    && let Some(kind) = self.classify_early_exit(if_stmt)
                {
                    match kind {
                        EarlyExitKind::UnreachableRest => {
                            after_dead += 1;
                            self.dead_depth += 1;
                        }
                        EarlyExitKind::Conditional {
                            cache_miss,
                            no_running_loop,
                        } => {
                            after_branch += 1;
                            self.branch_depth += 1;
                            if cache_miss {
                                after_cache += 1;
                                self.cache_miss_depth += 1;
                            }
                            if no_running_loop {
                                after_no_loop += 1;
                                self.no_running_loop_depth += 1;
                            }
                        }
                    }
                }
            } else {
                self.visit_stmt(stmt);
            }
        }
        self.branch_depth -= after_branch;
        self.cache_miss_depth -= after_cache;
        self.no_running_loop_depth -= after_no_loop;
        self.dead_depth -= after_dead;
    }

    pub(crate) fn classify_early_exit(&self, if_stmt: &ast::StmtIf) -> Option<EarlyExitKind> {
        if !body_always_exits(&if_stmt.body) {
            return None;
        }
        // Skip complex elif chains for cheap classification — still mark conditional.
        let has_elif = if_stmt.elif_else_clauses.iter().any(|c| c.test.is_some());
        if self.is_dead_branch_test(&if_stmt.test) {
            // `if False: return` — does not constrain subsequent code.
            return None;
        }
        if self.is_always_truthy(&if_stmt.test) {
            // No live else that continues: rest is unreachable.
            let else_continues = if_stmt
                .elif_else_clauses
                .iter()
                .any(|c| c.test.is_none() && !body_always_exits(&c.body) && !c.body.is_empty());
            if else_continues {
                return Some(EarlyExitKind::Conditional {
                    cache_miss: false,
                    no_running_loop: false,
                });
            }
            return Some(EarlyExitKind::UnreachableRest);
        }
        // `if loop is not None: return …` / `if loop.is_running(): return …`
        // → subsequent statements only run with no running event loop.
        let no_running_loop = self.is_running_loop_present_test_ctx(&if_stmt.test);
        if has_elif {
            return Some(EarlyExitKind::Conditional {
                cache_miss: false,
                no_running_loop,
            });
        }
        // Loop-state early returns must not be mis-tagged as cache-miss
        // (`is_cache_hit_test` also matches `x is not None`).
        let cache_miss = !no_running_loop && is_cache_hit_test(&if_stmt.test);
        Some(EarlyExitKind::Conditional {
            cache_miss,
            no_running_loop,
        })
    }

    /// Confirm `name` binds to a once-assigned module-level constant via ty.
    /// Returns `None` when unsure (local/param/global/import/other file).
    pub(crate) fn binds_to_module_const(&self, name_expr: &ast::ExprName) -> Option<ModuleConst> {
        let name = name_expr.id.as_str();
        let expected = *self.module_constants.get(name)?;
        if self.current_param_names.iter().any(|p| p == name) {
            return None;
        }
        let resolved = definitions_for_name(
            self.model,
            name,
            name_expr.into(),
            ImportAliasResolution::ResolveAliases,
        );
        if resolved.is_empty() {
            return None;
        }
        for r in &resolved {
            let def = r.definition()?;
            if !def.file_scope(self.model.db()).is_global() {
                return None;
            }
            match def.kind(self.model.db()) {
                DefinitionKind::Assignment(_) | DefinitionKind::AnnotatedAssignment(_) => {}
                _ => return None,
            }
            let path = def.file(self.model.db()).path(self.model.db());
            let FilePath::System(sys) = path else {
                return None;
            };
            let rel = relative_display(self.ctx.db.project_root(), sys);
            if rel != self.ctx.rel {
                // Imported binding — do not prune on name alone.
                return None;
            }
        }
        Some(expected)
    }

    pub(crate) fn is_dead_branch_test(&self, test: &Expr) -> bool {
        match test {
            Expr::BooleanLiteral(b) => !b.value,
            Expr::NoneLiteral(_) => true,
            Expr::Name(name) if name.id.as_str() == "TYPE_CHECKING" => true,
            Expr::Attribute(attr) if attr.attr.as_str() == "TYPE_CHECKING" => true,
            Expr::Name(name) => matches!(
                self.binds_to_module_const(name),
                Some(ModuleConst::False | ModuleConst::None | ModuleConst::Empty)
            ),
            Expr::UnaryOp(u) if matches!(u.op, ast::UnaryOp::Not) => match u.operand.as_ref() {
                Expr::BooleanLiteral(b) => b.value,
                Expr::Name(n) => {
                    matches!(self.binds_to_module_const(n), Some(ModuleConst::True))
                }
                _ => false,
            },
            _ => false,
        }
    }

    pub(crate) fn is_always_truthy(&self, test: &Expr) -> bool {
        match test {
            Expr::BooleanLiteral(b) => b.value,
            Expr::Name(n) => matches!(self.binds_to_module_const(n), Some(ModuleConst::True)),
            Expr::UnaryOp(u) if matches!(u.op, ast::UnaryOp::Not) => {
                self.is_dead_branch_test(u.operand.as_ref())
            }
            _ => false,
        }
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
            self.consuming_generator = true;
            self.visit_expr(&comp.iter);
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

    pub(crate) fn visit_if(&mut self, if_stmt: &'a ast::StmtIf) {
        let dead = self.is_dead_branch_test(&if_stmt.test);
        let always = self.is_always_truthy(&if_stmt.test);
        let cache_miss = !dead && is_cache_miss_test(&if_stmt.test);
        let no_loop = !dead
            && (self.is_no_running_loop_test_ctx(&if_stmt.test)
                || self.is_idle_loop_local_guard(&if_stmt.test));
        let off_loop = !dead && is_off_loop_thread_test(&if_stmt.test);
        let on_loop_thread = !dead && is_on_loop_thread_test(&if_stmt.test);
        let mut saw_running_loop_test = !dead && self.is_running_loop_test_ctx(&if_stmt.test);
        let mut saw_on_loop_thread_test = on_loop_thread;
        let done_names = if dead {
            Vec::new()
        } else {
            futures_proven_done_in_test(&if_stmt.test)
        };
        let param_guard = if dead {
            None
        } else {
            param_guard_from_test(&if_stmt.test, &self.current_param_names)
        };

        if dead {
            self.dead_depth += 1;
        } else {
            self.visit_expr(&if_stmt.test);
        }
        let prev_done = if !dead && !done_names.is_empty() {
            let prev = self.future_done_receivers.clone();
            for n in &done_names {
                self.future_done_receivers.insert(n.clone());
            }
            Some(prev)
        } else {
            None
        };
        if !dead {
            self.branch_depth += 1;
            if cache_miss {
                self.cache_miss_depth += 1;
            }
            if no_loop {
                self.no_running_loop_depth += 1;
            }
            if off_loop {
                self.off_loop_thread_depth += 1;
            }
        }
        let prev_guard = self.active_param_guard;
        if let Some(g) = param_guard {
            self.active_param_guard = Some(g);
        }
        let pushed_narrow = if !dead {
            self.push_narrowing(&if_stmt.test)
        } else {
            None
        };
        self.visit_body_seq(&if_stmt.body);
        self.pop_narrowing(pushed_narrow);
        self.active_param_guard = prev_guard;
        if let Some(prev) = prev_done {
            self.future_done_receivers = prev;
        }
        if !dead {
            if off_loop {
                self.off_loop_thread_depth -= 1;
            }
            if no_loop {
                self.no_running_loop_depth -= 1;
            }
            if cache_miss {
                self.cache_miss_depth -= 1;
            }
            self.branch_depth -= 1;
        }
        if dead {
            self.dead_depth -= 1;
        }

        for clause in &if_stmt.elif_else_clauses {
            let is_else = clause.test.is_none();
            let elif_dead = if is_else {
                // else of a provably-true if is unreachable.
                always
            } else {
                clause
                    .test
                    .as_ref()
                    .is_some_and(|t| self.is_dead_branch_test(t))
            };
            let elif_cache = clause
                .test
                .as_ref()
                .is_some_and(|t| !elif_dead && is_cache_miss_test(t));
            let elif_no_loop = if elif_dead {
                false
            } else if is_else {
                // `else` after a positive `loop.is_running()` / helper test means
                // no running loop. Do *not* treat else of bare `loop is None` as
                // a no-loop guard (that branch still has a loop).
                saw_running_loop_test
            } else if let Some(t) = clause.test.as_ref() {
                if self.is_running_loop_test_ctx(t) {
                    saw_running_loop_test = true;
                }
                self.is_no_running_loop_test_ctx(t) || self.is_idle_loop_local_guard(t)
            } else {
                false
            };
            let elif_off_loop = if elif_dead {
                false
            } else if is_else {
                // else of `current_thread() is main_thread()` → off-loop body.
                saw_on_loop_thread_test
            } else if let Some(t) = clause.test.as_ref() {
                if is_on_loop_thread_test(t) {
                    saw_on_loop_thread_test = true;
                }
                is_off_loop_thread_test(t)
            } else {
                false
            };
            let elif_done = if elif_dead {
                Vec::new()
            } else {
                clause
                    .test
                    .as_ref()
                    .map(futures_proven_done_in_test)
                    .unwrap_or_default()
            };
            let elif_guard = clause.test.as_ref().and_then(|t| {
                if elif_dead {
                    None
                } else {
                    param_guard_from_test(t, &self.current_param_names)
                }
            });
            if elif_dead {
                self.dead_depth += 1;
            } else if let Some(test) = &clause.test {
                self.visit_expr(test);
            }
            let prev_done = if !elif_dead && !elif_done.is_empty() {
                let prev = self.future_done_receivers.clone();
                for n in &elif_done {
                    self.future_done_receivers.insert(n.clone());
                }
                Some(prev)
            } else {
                None
            };
            // else / elif always counts as a branch when live — never unconditional.
            if !elif_dead {
                self.branch_depth += 1;
                if elif_cache {
                    self.cache_miss_depth += 1;
                }
                if elif_no_loop {
                    self.no_running_loop_depth += 1;
                }
                if elif_off_loop {
                    self.off_loop_thread_depth += 1;
                }
            }
            let prev_guard = self.active_param_guard;
            if let Some(g) = elif_guard {
                self.active_param_guard = Some(g);
            }
            let pushed_narrow = if !elif_dead {
                clause.test.as_ref().and_then(|t| self.push_narrowing(t))
            } else {
                None
            };
            self.visit_body_seq(&clause.body);
            self.pop_narrowing(pushed_narrow);
            self.active_param_guard = prev_guard;
            if let Some(prev) = prev_done {
                self.future_done_receivers = prev;
            }
            if !elif_dead {
                if elif_off_loop {
                    self.off_loop_thread_depth -= 1;
                }
                if elif_no_loop {
                    self.no_running_loop_depth -= 1;
                }
                if elif_cache {
                    self.cache_miss_depth -= 1;
                }
                self.branch_depth -= 1;
            }
            if elif_dead {
                self.dead_depth -= 1;
            }
        }
    }

    /// Apply one cheap narrowing for the duration of a branch body, returning
    /// the shadowed previous fact (if any) so `pop_narrowing` restores it.
    fn push_narrowing(
        &mut self,
        test: &Expr,
    ) -> Option<(String, Option<crate::extract::helpers::NarrowFact>)> {
        let (name, mut fact) = narrow_from_test(test)?;
        // Don't overwrite a tighter isinstance with NotNone.
        if matches!(
            (&fact, self.narrowed_locals.get(&name)),
            (
                crate::extract::helpers::NarrowFact::NotNone,
                Some(crate::extract::helpers::NarrowFact::IsInstance(_))
            )
        ) {
            return None;
        }
        // Resolve `isinstance(x, Redis)` / `isinstance(x, redis.Redis)` to a
        // stable instance qualname when ty can name the class.
        if let crate::extract::helpers::NarrowFact::IsInstance(_) = &fact
            && let Expr::Call(call) = test
            && let Some(type_expr) = call.arguments.args.get(1)
            && let Some(q) = self
                .type_instance_qualname(type_expr)
                .or_else(|| self.external_qualname(type_expr))
        {
            fact = crate::extract::helpers::NarrowFact::IsInstance(q);
        }
        let prev = self.narrowed_locals.insert(name.clone(), fact);
        Some((name, prev))
    }

    fn pop_narrowing(
        &mut self,
        pushed: Option<(String, Option<crate::extract::helpers::NarrowFact>)>,
    ) {
        if let Some((key, prev)) = pushed {
            match prev {
                Some(old) => {
                    self.narrowed_locals.insert(key, old);
                }
                None => {
                    self.narrowed_locals.remove(&key);
                }
            }
        }
    }

    pub(crate) fn visit_try(&mut self, try_stmt: &'a ast::StmtTry) {
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
                if let Expr::Call(call) = &item.context_expr {
                    let is_offload = self.handle_call(call, true);
                    self.walk_call_children(call, is_offload);
                } else {
                    self.visit_expr(&item.context_expr);
                }
                self.note_async_with_receiver(item);
            } else if !matches!(item.context_expr, Expr::Call(_)) {
                // Sync `with Call(...)` already recorded above as WithEnter.
                self.visit_expr(&item.context_expr);
            }
            // Bind `with ZipFile(buf) as zf` / buffer receivers for later method skips.
            if let Some(Expr::Name(n)) = item.optional_vars.as_deref() {
                self.note_local_binding(n.id.as_str(), &item.context_expr);
            }
            if let Some(opt) = &item.optional_vars {
                self.visit_expr(opt);
            }
        }
        self.visit_body_seq(&with_stmt.body);
    }
}
