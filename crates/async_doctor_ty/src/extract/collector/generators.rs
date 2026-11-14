//! Deferred local generator calls, resolved through ty's reaching definitions.

use async_doctor_core::{CallKind, CalleeRef, OffloadKind};
use ruff_db::parsed::parsed_module;
use ruff_python_ast::{self as ast, Expr};
use ruff_text_size::{Ranged, TextRange};
use ty_python_core::definition::DefinitionKind;

use super::CallCollector;
use super::async_anti_patterns::decorator_preserves_coroutine;
use crate::extract::constants::is_generator_consumer;

impl<'a, 'db> CallCollector<'a, 'db> {
    pub(super) fn is_exhaustive_generator_consumer(&mut self, func: &Expr) -> bool {
        is_generator_consumer(func)
            && self.external_qualname(func).is_some_and(|q| {
                matches!(
                    q.as_str(),
                    "builtins.list"
                        | "builtins.tuple"
                        | "builtins.set"
                        | "builtins.frozenset"
                        | "builtins.sorted"
                )
            })
    }

    /// Do not replay creation arguments: they already ran at the assignment.
    /// Reaching definitions handle aliases, rebinding, and ambiguous branches.
    pub(super) fn consume_named_generator(&mut self, expr: &Expr, exhausts: bool) {
        if !matches!(expr, Expr::Name(_))
            || self.current_fn.is_none()
            || self.dead_depth > 0
            || self.in_lambda
            || self.off_loop_thread_depth > 0
        {
            return;
        }
        if let Some(source) = self.follow_generator_source(expr, expr.range(), 8, true)
            && exhausts
            && self.branch_depth == 0
            && self.loop_depth == 0
        {
            self.exhausted_generators.insert(source);
        }
    }

    /// Awaiting `to_thread(list, rows)` exhausts rows on the worker. A later
    /// loop-side `list(rows)` cannot execute that generator body again.
    pub(super) fn note_offloaded_generator_consumption(
        &mut self,
        call: &ast::ExprCall,
        kind: Option<OffloadKind>,
    ) {
        let Some(kind) = kind else { return };
        if self.branch_depth > 0 || self.loop_depth > 0 {
            return;
        }
        let args = &call.arguments.args;
        let mut index = kind.callable_arg_index();
        if args
            .get(index)
            .is_some_and(|a| self.is_context_run_callable(a))
        {
            index += 1;
        }
        if args.len() != index + 2 || !call.arguments.keywords.is_empty() {
            return;
        }
        if self.is_exhaustive_generator_consumer(&args[index])
            && let Some(source) =
                self.follow_generator_source(&args[index + 1], call.range(), 8, false)
        {
            self.exhausted_generators.insert(source);
        }
    }

    fn follow_generator_source(
        &mut self,
        expr: &Expr,
        site: TextRange,
        remaining: u8,
        record_edge: bool,
    ) -> Option<TextRange> {
        if remaining == 0 {
            return None;
        }
        if let Expr::Name(name) = expr {
            if self.nonlocal_or_global_names.contains(name.id.as_str()) {
                return None;
            }
            let def = self.local_definition_at_use(name)?;
            let db = self.model.db();
            let parsed = parsed_module(db, def.python_file(db)).load(db);
            let value = match def.kind(db) {
                DefinitionKind::Assignment(a) if a.unpack().is_none() => a.value(&parsed),
                DefinitionKind::AnnotatedAssignment(a) => a.value(&parsed)?,
                _ => return None,
            };
            return self.follow_generator_source(value, site, remaining - 1, record_edge);
        }

        let (func, arguments, offset) = match expr {
            Expr::Call(call) => (call.func.as_ref(), &call.arguments, 0),
            Expr::Await(a) => {
                let Expr::Call(call) = a.value.as_ref() else {
                    return None;
                };
                let kind = self.is_offload_callee(&call.func)?;
                let mut index = kind.callable_arg_index();
                if self.is_context_run_callable(call.arguments.args.get(index)?) {
                    index += 1;
                }
                (call.arguments.args.get(index)?, &call.arguments, index + 1)
            }
            _ => return None,
        };
        let fid = self.resolve_project_function(func)?;
        let function = self.function(fid);
        if !function.is_generator
            || function.is_async
            || self.exhausted_generators.contains(&expr.range())
        {
            return None;
        }
        // A return-changing decorator can turn a generator into a context
        // manager or an arbitrary value; never infer consumption through it.
        for def in self.resolve_callee_function_defs(func)? {
            let DefinitionKind::Function(fdef) = def.kind(self.model.db()) else {
                return None;
            };
            let parsed = parsed_module(self.model.db(), def.python_file(self.model.db()))
                .load(self.model.db());
            if !fdef
                .node(&parsed)
                .decorator_list
                .iter()
                .all(|d| decorator_preserves_coroutine(&d.expression))
            {
                return None;
            }
        }
        if record_edge {
            let caller = self.current_fn?;
            self.clear_pending_call_args();
            self.pending_literals = arguments
                .args
                .iter()
                .skip(offset)
                .map(|a| self.call_arg_literal(a))
                .collect();
            self.pending_keyword_literals = self.call_keyword_literals(&arguments.keywords);
            self.pending_callables = arguments
                .args
                .iter()
                .skip(offset)
                .map(|a| self.resolve_project_function(a))
                .collect();
            self.pending_keyword_callables = arguments
                .keywords
                .iter()
                .filter_map(|kw| {
                    Some((
                        kw.arg.as_ref()?.as_str().to_string(),
                        self.resolve_project_function(&kw.value)?,
                    ))
                })
                .collect();
            self.pending_is_bound_call = self.expr_is_bound_method_call(func, fid);
            self.pending_is_awaited = false;
            self.push_edge(
                caller,
                CalleeRef::Project(fid),
                site,
                CallKind::GeneratorConsume,
                false,
                Some(false),
            );
        }
        Some(expr.range())
    }
}
