//! Pass-2 AST seed detection for high-precision LLM async correctness advice:
//! - `offload-stateful-session`
//! - `useless-local-async-lock` (see `local_lock` submodule)
//!
//! `swallowed-cancelled-error` was withdrawn for noise; see
//! `docs/decisions/2026-11-17-withdraw-swallowed-cancelled-error.md`.

mod local_lock;

use std::collections::HashSet;

use async_doctor_core::{CallKind, CalleeRef, FunctionId, OffloadKind};
use ruff_python_ast::Expr;
use ruff_text_size::TextRange;
use ty_python_semantic::HasType;
use ty_python_semantic::types::{Type, TypeDefinition};

use super::CallCollector;
use crate::extract::helpers::module_of_definition;

fn canonical_sync_db_handle(qualname: &str) -> Option<&'static str> {
    match qualname {
        "sqlalchemy.orm.Session" | "sqlalchemy.orm.session.Session" => {
            Some("sqlalchemy.orm.Session")
        }
        "sqlmodel.Session" | "sqlmodel.orm.session.Session" => Some("sqlmodel.Session"),
        "sqlite3.Connection" | "_sqlite3.Connection" => Some("sqlite3.Connection"),
        "sqlite3.Cursor" | "_sqlite3.Cursor" => Some("sqlite3.Cursor"),
        _ => None,
    }
}

fn canonical_sync_db_handle_from_method(qualname: &str) -> Option<&'static str> {
    let (owner, _method) = qualname.rsplit_once('.')?;
    canonical_sync_db_handle(owner)
}

impl<'a, 'db> CallCollector<'a, 'db> {
    // ── offload-stateful-session ────────────────────────────────────────────

    pub(crate) fn maybe_record_offload_stateful_session(
        &mut self,
        caller: FunctionId,
        effective_target: Option<&'a Expr>,
        kind: OffloadKind,
        offload_range: TextRange,
    ) {
        if self.function(caller).is_installed_third_party() {
            return;
        }
        let Some(target) = effective_target else {
            return;
        };
        let (callable_expr, lambda_params) = if let Some(lam) = self.resolve_lambda_expr(target) {
            let Expr::Call(inner_call) = lam.body.as_ref() else {
                return;
            };
            let params: HashSet<&str> = lam
                .parameters
                .as_ref()
                .map(|p| {
                    p.iter_non_variadic_params()
                        .map(|a| a.parameter.name.as_str())
                        .collect()
                })
                .unwrap_or_default();
            (inner_call.func.as_ref(), Some(params))
        } else {
            (target, None)
        };

        let Expr::Attribute(attr) = callable_expr else {
            return;
        };
        if let Some(params) = &lambda_params {
            // Connection/Session constructed inside the worker lambda (`lambda: sqlite3.connect(...).execute(...)`)
            // runs wholly on the worker thread and does not cross threads.
            match attr.value.as_ref() {
                Expr::Name(n) if !params.contains(n.id.as_str()) => {}
                Expr::Attribute(_) => {}
                _ => return,
            }
        }
        if matches!(
            attr.value.inferred_type(self.model),
            Some(Type::ClassLiteral(_) | Type::SubclassOf(_))
        ) {
            return;
        }
        if !self.expr_is_external_bound_call(callable_expr) {
            return;
        }
        let Some(handle_name) = self.stateful_sync_db_handle_of(&attr.value, callable_expr) else {
            return;
        };
        self.clear_pending_call_args();
        self.push_edge(
            caller,
            CalleeRef::External {
                qualname: format!("offload-stateful-session:{}:{handle_name}", kind.as_str()),
            },
            offload_range,
            CallKind::Call,
            false,
            Some(false),
        );
    }

    fn stateful_sync_db_handle_of(
        &mut self,
        recv: &Expr,
        callable_expr: &Expr,
    ) -> Option<&'static str> {
        if let Some(ty) = recv.inferred_type(self.model) {
            let env = self.model.program_environment();
            if let Some(h) = self.type_to_sync_db_handle(ty, &env) {
                return Some(h);
            }
        }
        let ext = self.external_qualname(callable_expr)?;
        canonical_sync_db_handle_from_method(&ext)
    }

    fn type_to_sync_db_handle(
        &self,
        ty: Type<'db>,
        env: &ty_python_semantic::ProgramEnvironment<'db>,
    ) -> Option<&'static str> {
        if matches!(
            ty,
            Type::ClassLiteral(_) | Type::SubclassOf(_) | Type::GenericAlias(_)
        ) {
            return None;
        }
        if let Some(union) = ty.as_union() {
            let mut matched: Option<&'static str> = None;
            for elem in union.elements(self.model.db()).iter() {
                if matches!(elem, Type::Never) || elem.is_none(self.model.db()) {
                    continue;
                }
                let h = self.type_to_sync_db_handle(*elem, env)?;
                matched = Some(matched.unwrap_or(h));
            }
            return matched;
        }
        let (TypeDefinition::StaticClass(def) | TypeDefinition::DynamicClass(def)) =
            ty.definition(self.model.db(), env)?
        else {
            return None;
        };
        let name = def.name(self.model.db())?;
        let module = module_of_definition(self.model, def)?;
        let qual = format!("{module}.{name}");
        canonical_sync_db_handle(&qual)
    }
}
