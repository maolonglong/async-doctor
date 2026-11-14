//! Execution edges for callbacks proven to be scheduled by asyncio.

use async_doctor_core::{CallKind, CalleeRef};
use ruff_python_ast::{self as ast, Expr};
use ruff_text_size::Ranged;
use ty_python_semantic::definitions_for_attribute;

use super::CallCollector;
use crate::extract::helpers::qualname_from_definition;

impl<'a, 'db> CallCollector<'a, 'db> {
    pub(super) fn maybe_record_asyncio_callback(
        &mut self,
        caller: async_doctor_core::FunctionId,
        call: &'a ast::ExprCall,
    ) {
        let Expr::Attribute(attr) = call.func.as_ref() else {
            return;
        };
        let (index, future_arg) = match attr.attr.as_str() {
            "call_soon" | "call_soon_threadsafe" => (0, false),
            "call_later" | "call_at" => (1, false),
            "add_done_callback" => (0, true),
            _ => return,
        };
        // The same method names on arbitrary objects (especially concurrent
        // futures) say nothing about where their callback will execute.
        let definitions = definitions_for_attribute(self.model, attr);
        if definitions.is_empty()
            || !definitions.iter().all(|r| {
                r.definition()
                    .and_then(|d| qualname_from_definition(self.model, d))
                    .and_then(|q| q.rsplit_once('.').map(|(class, _)| class.to_string()))
                    .is_some_and(|class| {
                        if future_arg {
                            matches!(
                                class.as_str(),
                                "_asyncio.Future"
                                    | "_asyncio.Task"
                                    | "asyncio.futures.Future"
                                    | "asyncio.tasks.Task"
                            )
                        } else {
                            matches!(
                                class.as_str(),
                                "asyncio.events.AbstractEventLoop"
                                    | "asyncio.base_events.BaseEventLoop"
                            )
                        }
                    })
            })
        {
            return;
        }
        let Some(mut target) = call.arguments.args.get(index).or_else(|| {
            (!future_arg).then(|| {
                call.arguments
                    .keywords
                    .iter()
                    .find(|k| k.arg.as_ref().is_some_and(|a| a.as_str() == "callback"))
                    .map(|k| &k.value)
            })?
        }) else {
            return;
        };
        let mut args = Vec::new();
        let mut keywords = Vec::new();
        for _ in 0..8 {
            match target {
                Expr::Call(partial) if self.is_partial_call_ctx(partial) => {
                    let Some(first) = partial.arguments.args.first() else {
                        return;
                    };
                    args.splice(0..0, partial.arguments.args.iter().skip(1));
                    keywords.splice(0..0, partial.arguments.keywords.iter());
                    target = first;
                }
                // Existing local-partial metadata retains only its target, not
                // bound argument values. Do not invent a callback binding.
                Expr::Name(n) if self.local_partials.contains_key(n.id.as_str()) => return,
                _ => break,
            }
        }
        args.extend(call.arguments.args.iter().skip(index + 1));
        if args.iter().any(|a| matches!(a, Expr::Starred(_)))
            || keywords.iter().any(|k| k.arg.is_none())
        {
            return;
        }
        let fid = if let Some(lambda) = self.resolve_lambda_expr(target) {
            Some(self.ensure_lambda_fact(lambda))
        } else {
            self.resolve_project_function(target)
        };
        if fid.is_some_and(|id| {
            let f = self.function(id);
            f.is_async || f.is_generator
        }) || self.resolve_callee_is_coroutine(target) == Some(true)
        {
            // Calling these only constructs a deferred object, never its body.
            return;
        }
        let callee = if let Some(fid) = fid {
            CalleeRef::Project(fid)
        } else if let Some(qualname) = self.external_qualname(target) {
            CalleeRef::External { qualname }
        } else {
            return;
        };
        self.clear_pending_call_args();
        self.pending_literals = args.iter().map(|a| self.call_arg_literal(a)).collect();
        self.pending_callables = args
            .iter()
            .map(|a| self.resolve_callable_project_fid(a))
            .collect();
        if future_arg {
            // Future/Task supplies itself after any partial-bound arguments.
            self.pending_literals.push(None);
            self.pending_callables.push(None);
        }
        for kw in keywords {
            let name = kw.arg.as_ref().unwrap().as_str().to_string();
            self.pending_keyword_literals.retain(|(k, _)| k != &name);
            let value = self.call_arg_literal(&kw.value);
            self.pending_keyword_literals.push((name.clone(), value));
            self.pending_keyword_callables.retain(|(k, _)| k != &name);
            if let Some(id) = self.resolve_callable_project_fid(&kw.value) {
                self.pending_keyword_callables.push((name, id));
            }
        }
        self.pending_is_bound_call =
            fid.is_some_and(|id| self.expr_is_bound_method_call(target, id));
        self.pending_is_awaited = false;
        self.push_edge(
            caller,
            callee,
            call.range(),
            CallKind::Call,
            false,
            Some(false),
        );
    }
}
