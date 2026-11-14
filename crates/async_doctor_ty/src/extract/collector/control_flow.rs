//! Control-flow-aware body walking for call collection.

mod loops_and_with;

use std::collections::HashSet;

use async_doctor_core::ConditionTag;
use ruff_db::files::FilePath;
use ruff_python_ast::visitor::Visitor;
use ruff_python_ast::{self as ast, Expr, Stmt};
use ty_python_core::definition::DefinitionKind;
use ty_python_semantic::{ImportAliasResolution, definitions_for_name};

use super::CallCollector;
use super::thread_guards::{
    futures_proven_done_in_test, is_off_loop_thread_test, is_on_loop_thread_test,
};
use crate::extract::constants::{
    EarlyExitKind, ModuleConst, body_always_exits, cache_hit_storage_operand,
    cache_miss_storage_operand, param_guards_from_test,
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
        let saved_guard_len = self.active_param_guards.len();
        for stmt in body {
            if let Stmt::If(if_stmt) = stmt {
                self.visit_if(if_stmt);
                if after_dead == 0 {
                    if let Some(kind) = self.classify_early_exit(if_stmt) {
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
                    self.push_early_exit_param_guards(if_stmt);
                }
            } else {
                self.visit_stmt(stmt);
            }
        }
        self.active_param_guards.truncate(saved_guard_len);
        self.branch_depth -= after_branch;
        self.cache_miss_depth -= after_cache;
        self.no_running_loop_depth -= after_no_loop;
        self.dead_depth -= after_dead;
    }

    fn push_unique_param_guard(&mut self, guard: async_doctor_core::ParamGuard) {
        if !self.active_param_guards.contains(&guard) {
            self.active_param_guards.push(guard);
        }
    }

    /// Record inverted parameter guards after `if <exact-param-test>: return/raise/...`
    /// (and contiguous `elif` branches that also always exit).
    fn push_early_exit_param_guards(&mut self, if_stmt: &ast::StmtIf) {
        if !body_always_exits(&if_stmt.body) {
            return;
        }
        if !self.is_dead_branch_test(&if_stmt.test) {
            let (guards, exact) = param_guards_from_test(&if_stmt.test, &self.current_param_names);
            if exact && guards.len() == 1 {
                self.push_unique_param_guard(guards[0].inverted());
            }
        }
        for clause in &if_stmt.elif_else_clauses {
            let Some(test) = &clause.test else {
                break;
            };
            if !body_always_exits(&clause.body) {
                break;
            }
            if !self.is_dead_branch_test(test) {
                let (guards, exact) = param_guards_from_test(test, &self.current_param_names);
                if exact && guards.len() == 1 {
                    self.push_unique_param_guard(guards[0].inverted());
                }
            }
        }
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
        let cache_miss = !no_running_loop && self.is_cache_hit_test_ctx(&if_stmt.test);
        Some(EarlyExitKind::Conditional {
            cache_miss,
            no_running_loop,
        })
    }

    fn is_cache_hit_test_ctx(&self, test: &Expr) -> bool {
        cache_hit_storage_operand(test).is_some_and(|s| self.is_cache_storage_expr(s))
    }

    fn is_cache_miss_test_ctx(&self, test: &Expr) -> bool {
        cache_miss_storage_operand(test).is_some_and(|s| self.is_cache_storage_expr(s))
    }

    /// True when `storage` refers to persistent cache state (attribute, subscript,
    /// `global`/`nonlocal` name, or module-scope variable) rather than a function
    /// parameter or ordinary local variable.
    fn is_cache_storage_expr(&self, storage: &Expr) -> bool {
        match storage {
            Expr::Attribute(_) | Expr::Subscript(_) => true,
            Expr::Name(name_expr) => {
                let name = name_expr.id.as_str();
                if self.current_param_names.iter().any(|p| p == name) {
                    return false;
                }
                if self.nonlocal_or_global_names.contains(name) {
                    return true;
                }
                let resolved = definitions_for_name(
                    self.model,
                    name,
                    name_expr.into(),
                    ImportAliasResolution::ResolveAliases,
                );
                !resolved.is_empty()
                    && resolved.iter().all(|r| {
                        r.definition()
                            .is_some_and(|def| def.file_scope(self.model.db()).is_global())
                    })
            }
            _ => false,
        }
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
                return None;
            }
        }
        Some(expected)
    }

    pub(crate) fn is_dead_branch_test(&self, test: &Expr) -> bool {
        match test {
            Expr::BooleanLiteral(b) => !b.value,
            Expr::Name(n) if n.id.as_str() == "TYPE_CHECKING" => true,
            Expr::Name(n) => matches!(
                self.binds_to_module_const(n),
                Some(ModuleConst::False | ModuleConst::Empty)
            ),
            Expr::Attribute(a) if a.attr.as_str() == "TYPE_CHECKING" => true,
            Expr::UnaryOp(u) if matches!(u.op, ast::UnaryOp::Not) => {
                self.is_always_truthy(&u.operand)
            }
            Expr::BoolOp(b) => match b.op {
                ast::BoolOp::And => b.values.iter().any(|v| self.is_dead_branch_test(v)),
                ast::BoolOp::Or => b.values.iter().all(|v| self.is_dead_branch_test(v)),
            },
            _ => false,
        }
    }

    pub(crate) fn is_always_truthy(&self, test: &Expr) -> bool {
        match test {
            Expr::BooleanLiteral(b) => b.value,
            Expr::Name(n) => matches!(self.binds_to_module_const(n), Some(ModuleConst::True)),
            Expr::UnaryOp(u) if matches!(u.op, ast::UnaryOp::Not) => {
                self.is_dead_branch_test(&u.operand)
            }
            _ => false,
        }
    }

    pub(crate) fn visit_if(&mut self, if_stmt: &'a ast::StmtIf) {
        let pre_local_guards = (
            self.in_memory_sqlite_vars.clone(),
            self.nonblocking_socket_vars.clone(),
            self.in_memory_archive_receivers.clone(),
        );
        let mut live_fallthrough_guards: Vec<(HashSet<String>, HashSet<String>, HashSet<String>)> =
            Vec::new();
        let mut has_else = false;

        let dead = self.is_dead_branch_test(&if_stmt.test);
        let always = self.is_always_truthy(&if_stmt.test);
        let cache_miss = !dead && self.is_cache_miss_test_ctx(&if_stmt.test);
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
        let (if_guards, if_exact) = if dead {
            (Vec::new(), false)
        } else {
            param_guards_from_test(&if_stmt.test, &self.current_param_names)
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
        let saved_guard_len = self.active_param_guards.len();
        for g in &if_guards {
            self.push_unique_param_guard(*g);
        }
        let pushed_narrow = if !dead {
            self.push_narrowing(&if_stmt.test)
        } else {
            None
        };
        self.visit_body_seq(&if_stmt.body);
        self.pop_narrowing(pushed_narrow);
        self.active_param_guards.truncate(saved_guard_len);
        let mut negated_prior: Vec<async_doctor_core::ParamGuard> = Vec::new();
        if !dead && if_exact && if_guards.len() == 1 {
            negated_prior.push(if_guards[0].inverted());
        }
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
        } else if !body_always_exits(&if_stmt.body) {
            live_fallthrough_guards.push((
                self.in_memory_sqlite_vars.clone(),
                self.nonblocking_socket_vars.clone(),
                self.in_memory_archive_receivers.clone(),
            ));
        }

        for clause in &if_stmt.elif_else_clauses {
            self.in_memory_sqlite_vars.clone_from(&pre_local_guards.0);
            self.nonblocking_socket_vars.clone_from(&pre_local_guards.1);
            self.in_memory_archive_receivers
                .clone_from(&pre_local_guards.2);
            let is_else = clause.test.is_none();
            if is_else {
                has_else = true;
            }
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
                .is_some_and(|t| !elif_dead && self.is_cache_miss_test_ctx(t));
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
            let (elif_guards, elif_exact) = match clause.test.as_ref() {
                Some(t) if !elif_dead => param_guards_from_test(t, &self.current_param_names),
                _ => (Vec::new(), false),
            };
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
            let clause_guard_len = self.active_param_guards.len();
            if !elif_dead {
                for g in &negated_prior {
                    self.push_unique_param_guard(*g);
                }
                for g in &elif_guards {
                    self.push_unique_param_guard(*g);
                }
            }
            let pushed_narrow = if !elif_dead {
                clause.test.as_ref().and_then(|t| self.push_narrowing(t))
            } else {
                None
            };
            self.visit_body_seq(&clause.body);
            self.pop_narrowing(pushed_narrow);
            self.active_param_guards.truncate(clause_guard_len);
            if !elif_dead && elif_exact && elif_guards.len() == 1 {
                negated_prior.push(elif_guards[0].inverted());
            }
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
            } else if !body_always_exits(&clause.body) {
                live_fallthrough_guards.push((
                    self.in_memory_sqlite_vars.clone(),
                    self.nonblocking_socket_vars.clone(),
                    self.in_memory_archive_receivers.clone(),
                ));
            }
        }

        if !has_else && !always {
            live_fallthrough_guards.push(pre_local_guards.clone());
        }
        if let Some((mut sql, mut sock, mut arch)) = live_fallthrough_guards.pop() {
            for (s, k, a) in &live_fallthrough_guards {
                sql.retain(|v| s.contains(v));
                sock.retain(|v| k.contains(v));
                arch.retain(|v| a.contains(v));
            }
            self.in_memory_sqlite_vars = sql;
            self.nonblocking_socket_vars = sock;
            self.in_memory_archive_receivers = arch;
        } else {
            self.in_memory_sqlite_vars = pre_local_guards.0;
            self.nonblocking_socket_vars = pre_local_guards.1;
            self.in_memory_archive_receivers = pre_local_guards.2;
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
}
