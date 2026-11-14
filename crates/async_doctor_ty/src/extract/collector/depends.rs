//! FastAPI / Litestar dependency injection (`Depends`, `Security`, `Provide`).

use async_doctor_core::{CallKind, CalleeRef, FunctionId, OffloadKind};
use ruff_python_ast::{self as ast, Expr};
use ruff_text_size::{Ranged, TextRange};

use super::CallCollector;
use crate::extract::functions::decorator_is_route;
use crate::extract::helpers::is_partial_call;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DiMarkerKind {
    /// FastAPI / Starlette `Depends` or `Security` (sync defs run in Starlette threadpool).
    FastApi,
    /// Litestar `Provide` (sync defs run in threadpool when `sync_to_thread=True`).
    Litestar,
}

impl<'a, 'db> CallCollector<'a, 'db> {
    /// Emit edges from `owner` to dependencies declared in `func.parameters`
    /// (`param = Depends(dep)` and `param: Annotated[T, Depends(dep)]`) and
    /// route decorator `dependencies=[Depends(dep)]`.
    pub(crate) fn note_function_dependencies(
        &mut self,
        owner: FunctionId,
        func: &'a ast::StmtFunctionDef,
    ) {
        if self.dead_depth > 0 {
            return;
        }
        for param in func.parameters.iter_non_variadic_params() {
            if let Some(ann) = param.parameter.annotation.as_deref() {
                self.visit_annotation_for_di(owner, ann);
            }
            if let Some(default) = param.default.as_deref() {
                self.visit_default_for_di(owner, default, param.parameter.annotation.as_deref());
            }
        }
        for deco in &func.decorator_list {
            if let Expr::Call(call) = &deco.expression
                && decorator_is_route(&call.func)
            {
                for kw in &call.arguments.keywords {
                    if kw
                        .arg
                        .as_ref()
                        .is_some_and(|a| a.as_str() == "dependencies")
                    {
                        self.visit_container_for_di(owner, &kw.value);
                    }
                }
            }
        }
    }

    fn visit_annotation_for_di(&mut self, owner: FunctionId, ann: &'a Expr) {
        let Expr::Subscript(sub) = ann else {
            return;
        };
        if !is_annotated_subscript(&sub.value) {
            return;
        }
        let Expr::Tuple(tuple) = sub.slice.as_ref() else {
            return;
        };
        if let Some(inner_type) = tuple.elts.first() {
            self.visit_annotation_for_di(owner, inner_type);
            for meta in tuple.elts.iter().skip(1) {
                self.visit_default_for_di(owner, meta, Some(inner_type));
            }
        }
    }

    fn visit_container_for_di(&mut self, owner: FunctionId, expr: &'a Expr) {
        match expr {
            Expr::List(l) => {
                for elt in &l.elts {
                    self.visit_default_for_di(owner, elt, None);
                }
            }
            Expr::Tuple(t) => {
                for elt in &t.elts {
                    self.visit_default_for_di(owner, elt, None);
                }
            }
            other => self.visit_default_for_di(owner, other, None),
        }
    }

    fn visit_default_for_di(
        &mut self,
        owner: FunctionId,
        expr: &'a Expr,
        param_type_ann: Option<&'a Expr>,
    ) {
        let Expr::Call(call) = expr else {
            return;
        };
        let Some(marker) = self.di_marker_kind(call) else {
            return;
        };
        let dep_expr = call
            .arguments
            .args
            .first()
            .or_else(|| {
                call.arguments
                    .keywords
                    .iter()
                    .find(|kw| kw.arg.as_ref().is_some_and(|a| a.as_str() == "dependency"))
                    .map(|kw| &kw.value)
            })
            .and_then(|e| (!matches!(e, Expr::NoneLiteral(_))).then_some(e))
            .or_else(|| {
                // FastAPI shorthand `svc: MyService = Depends()` uses the parameter type.
                (marker == DiMarkerKind::FastApi)
                    .then_some(param_type_ann.map(unwrap_annotated_base_type))
                    .flatten()
            });
        let Some(dep_expr) = dep_expr else {
            return;
        };
        let target = match dep_expr {
            Expr::Call(inner) if is_partial_call(inner) => {
                let Some(first) = inner.arguments.args.first() else {
                    return;
                };
                first
            }
            other => other,
        };
        self.emit_di_edge(owner, marker, call, target, call.range());
    }

    fn di_marker_kind(&mut self, call: &ast::ExprCall) -> Option<DiMarkerKind> {
        if let Some(q) = self.external_qualname(&call.func) {
            return match q.as_str() {
                "fastapi.Depends"
                | "fastapi.params.Depends"
                | "fastapi.param_functions.Depends"
                | "fastapi.Security"
                | "fastapi.params.Security"
                | "fastapi.param_functions.Security" => Some(DiMarkerKind::FastApi),
                "litestar.di.Provide" | "litestar.Provide" => Some(DiMarkerKind::Litestar),
                _ => None,
            };
        }
        // Fallback when fastapi/litestar is not installed in `.venv`: match
        // imported `Depends` / `Security` / `Provide` only if it is not a project def.
        if self.resolve_project_function(&call.func).is_some()
            || !self.resolve_project_constructors(&call.func).is_empty()
        {
            return None;
        }
        match call.func.as_ref() {
            Expr::Name(n) => match n.id.as_str() {
                "Depends" | "Security" => Some(DiMarkerKind::FastApi),
                "Provide" => Some(DiMarkerKind::Litestar),
                _ => None,
            },
            Expr::Attribute(a) => match a.attr.as_str() {
                "Depends" | "Security" => Some(DiMarkerKind::FastApi),
                "Provide" => Some(DiMarkerKind::Litestar),
                _ => None,
            },
            _ => None,
        }
    }

    fn emit_di_edge(
        &mut self,
        owner: FunctionId,
        marker: DiMarkerKind,
        marker_call: &ast::ExprCall,
        target: &'a Expr,
        range: TextRange,
    ) {
        self.clear_pending_call_args();
        let proj_fid = self
            .resolve_project_function(target)
            .or_else(|| self.resolve_project_constructors(target).into_iter().next());
        if let Some(dep_fid) = proj_fid {
            if self.function(dep_fid).is_async {
                let kind = if self.function(dep_fid).is_generator {
                    CallKind::GeneratorConsume
                } else {
                    CallKind::Call
                };
                self.pending_is_awaited = true;
                self.push_edge(
                    owner,
                    CalleeRef::Project(dep_fid),
                    range,
                    kind,
                    false,
                    Some(true),
                );
            } else if marker == DiMarkerKind::FastApi || litestar_sync_to_thread_true(marker_call) {
                self.pending_callables = vec![Some(dep_fid)];
                self.pending_is_awaited = true;
                self.push_edge(
                    owner,
                    CalleeRef::Offload(OffloadKind::StarletteThreadpool),
                    range,
                    CallKind::Call,
                    true,
                    Some(false),
                );
            } else {
                let kind = if self.function(dep_fid).is_generator {
                    CallKind::GeneratorConsume
                } else {
                    CallKind::Call
                };
                self.push_edge(
                    owner,
                    CalleeRef::Project(dep_fid),
                    range,
                    kind,
                    false,
                    Some(false),
                );
            }
            self.clear_pending_call_args();
            return;
        }

        let ext_qual = self.external_qualname(target);
        if let Some(callee) = self.resolve_non_project_callee(target, ext_qual) {
            let is_coro = self.resolve_callee_is_coroutine(target);
            self.pending_is_awaited = is_coro == Some(true);
            self.push_edge(owner, callee, range, CallKind::Call, false, is_coro);
        }
        self.clear_pending_call_args();
    }
}

fn is_annotated_subscript(expr: &Expr) -> bool {
    match expr {
        Expr::Name(n) => n.id.as_str() == "Annotated",
        Expr::Attribute(a) => a.attr.as_str() == "Annotated",
        _ => false,
    }
}

fn unwrap_annotated_base_type(ann: &Expr) -> &Expr {
    if let Expr::Subscript(sub) = ann
        && is_annotated_subscript(&sub.value)
        && let Expr::Tuple(t) = sub.slice.as_ref()
        && let Some(first) = t.elts.first()
    {
        return unwrap_annotated_base_type(first);
    }
    ann
}

fn litestar_sync_to_thread_true(call: &ast::ExprCall) -> bool {
    call.arguments.keywords.iter().any(|kw| {
        kw.arg
            .as_ref()
            .is_some_and(|a| a.as_str() == "sync_to_thread")
            && matches!(&kw.value, Expr::BooleanLiteral(b) if b.value)
    })
}
