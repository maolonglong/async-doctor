//! `eager-offload-call` detection during pass-2 AST collection.
//!
//! Fires when an offload helper's callable-slot argument is a `Call` expression
//! (`to_thread(fn(...))`, excluding `functools.partial`) whose return value is
//! provably non-callable (`None`, literal/builtin scalar or collection,
//! coroutine/generator, or base-free project class without `__call__` / `__new__`).

use async_doctor_core::{CallKind, CalleeRef, FunctionId, OffloadKind};
use ruff_db::files::FilePath;
use ruff_db::parsed::parsed_module;
use ruff_python_ast::{self as ast, Expr, Stmt};
use ruff_text_size::{Ranged, TextRange};
use ty_python_core::definition::{Definition, DefinitionKind};
use ty_python_semantic::HasType;
use ty_python_semantic::types::{Type, TypeDefinition};

use super::CallCollector;
use super::async_anti_patterns::decorator_preserves_coroutine;
use crate::extract::constants::body_has_yield;
use crate::extract::helpers::module_of_definition;

fn is_known_non_callable_instance_qual(qualname: &str) -> bool {
    matches!(
        qualname,
        "None"
            | "builtins.NoneType"
            | "types.NoneType"
            | "builtins.bool"
            | "builtins.int"
            | "builtins.float"
            | "builtins.complex"
            | "builtins.str"
            | "builtins.bytes"
            | "builtins.bytearray"
            | "builtins.memoryview"
            | "builtins.list"
            | "builtins.dict"
            | "builtins.set"
            | "builtins.frozenset"
            | "builtins.tuple"
            | "builtins.range"
            | "builtins.slice"
            | "builtins.generator"
            | "builtins.coroutine"
            | "builtins.async_generator"
            | "types.CoroutineType"
            | "types.GeneratorType"
            | "types.AsyncGeneratorType"
            | "pathlib.Path"
            | "pathlib.PosixPath"
            | "pathlib.WindowsPath"
            | "pathlib.PurePath"
            | "datetime.date"
            | "datetime.datetime"
            | "datetime.time"
            | "datetime.timedelta"
            | "uuid.UUID"
            | "decimal.Decimal"
            | "re.Match"
            | "httpx.Response"
            | "httpx._models.Response"
            | "requests.models.Response"
            | "requests.Response"
            | "urllib3.response.HTTPResponse"
            | "subprocess.CompletedProcess"
    )
}

/// True when every `return` statement inside `body` (excluding nested defs/classes/lambdas)
/// returns `None` or a literal scalar/collection that is provably non-callable.
fn fn_body_returns_only_non_callable_literals(body: &[Stmt]) -> bool {
    fn is_non_callable_literal_expr(expr: &Expr) -> bool {
        match expr {
            Expr::NoneLiteral(_)
            | Expr::BooleanLiteral(_)
            | Expr::NumberLiteral(_)
            | Expr::StringLiteral(_)
            | Expr::BytesLiteral(_)
            | Expr::FString(_)
            | Expr::List(_)
            | Expr::Dict(_)
            | Expr::Set(_)
            | Expr::Tuple(_)
            | Expr::ListComp(_)
            | Expr::DictComp(_)
            | Expr::SetComp(_)
            | Expr::Generator(_) => true,
            Expr::UnaryOp(u) => is_non_callable_literal_expr(&u.operand),
            _ => false,
        }
    }

    fn check_stmts(stmts: &[Stmt]) -> bool {
        for stmt in stmts {
            match stmt {
                Stmt::Return(r) => match r.value.as_deref() {
                    None => {}
                    Some(v) if is_non_callable_literal_expr(v) => {}
                    Some(_) => return false,
                },
                Stmt::If(s) => {
                    if !check_stmts(&s.body)
                        || !s.elif_else_clauses.iter().all(|c| check_stmts(&c.body))
                    {
                        return false;
                    }
                }
                Stmt::For(s) => {
                    if !check_stmts(&s.body) || !check_stmts(&s.orelse) {
                        return false;
                    }
                }
                Stmt::While(s) => {
                    if !check_stmts(&s.body) || !check_stmts(&s.orelse) {
                        return false;
                    }
                }
                Stmt::With(s) => {
                    if !check_stmts(&s.body) {
                        return false;
                    }
                }
                Stmt::Try(s) => {
                    if !check_stmts(&s.body)
                        || !s.handlers.iter().all(|h| {
                            let ast::ExceptHandler::ExceptHandler(h) = h;
                            check_stmts(&h.body)
                        })
                        || !check_stmts(&s.orelse)
                        || !check_stmts(&s.finalbody)
                    {
                        return false;
                    }
                }
                Stmt::Match(s) if !s.cases.iter().all(|c| check_stmts(&c.body)) => {
                    return false;
                }
                _ => {}
            }
        }
        true
    }

    // Reject empty stub/abstract bodies (`...` or `raise NotImplementedError`) so
    // abstract hooks whose subclasses might return callables are never guessed.
    let is_trivial_stub = match body {
        [] => true,
        [Stmt::Expr(e)] => matches!(
            e.value.as_ref(),
            Expr::EllipsisLiteral(_) | Expr::StringLiteral(_)
        ),
        [Stmt::Pass(_)] | [Stmt::Raise(_)] => true,
        [Stmt::Expr(doc), Stmt::Pass(_) | Stmt::Raise(_)]
            if matches!(doc.value.as_ref(), Expr::StringLiteral(_)) =>
        {
            true
        }
        [Stmt::Expr(doc), Stmt::Expr(e)]
            if matches!(doc.value.as_ref(), Expr::StringLiteral(_))
                && matches!(e.value.as_ref(), Expr::EllipsisLiteral(_)) =>
        {
            true
        }
        _ => false,
    };
    if is_trivial_stub {
        return false;
    }
    check_stmts(body)
}

fn class_node_is_non_callable_instance(class_node: &ast::StmtClassDef) -> bool {
    if !class_node.bases().is_empty() {
        return false;
    }
    if class_node
        .arguments
        .as_ref()
        .is_some_and(|args| !args.keywords.is_empty())
    {
        return false;
    }
    for stmt in &class_node.body {
        match stmt {
            Stmt::FunctionDef(f) if matches!(f.name.as_str(), "__call__" | "__new__") => {
                return false;
            }
            Stmt::Assign(a)
                if a.targets
                    .iter()
                    .any(|t| matches!(t, Expr::Name(n) if n.id.as_str() == "__call__")) =>
            {
                return false;
            }
            _ => {}
        }
    }
    true
}

impl<'a, 'db> CallCollector<'a, 'db> {
    /// Record an `eager-offload-call` edge when an offload helper's callable
    /// argument is a `Call` expression returning a provably non-callable value.
    pub(crate) fn maybe_record_eager_offload_call(
        &mut self,
        caller: FunctionId,
        offload_call: &ast::ExprCall,
        kind: OffloadKind,
        offload_range: TextRange,
    ) {
        if self.function(caller).is_installed_third_party() {
            return;
        }
        let callable_idx = kind.callable_arg_index();
        let target_idx = if offload_call
            .arguments
            .args
            .get(callable_idx)
            .is_some_and(|a| self.is_context_run_callable(a))
        {
            callable_idx + 1
        } else {
            callable_idx
        };
        let Some(arg) = offload_call.arguments.args.get(target_idx) else {
            return;
        };
        let Expr::Call(inner) = arg else {
            return;
        };
        if self.is_partial_call_ctx(inner) {
            return;
        }
        if !self.call_arg_returns_provably_non_callable(arg, inner) {
            return;
        }
        let inner_line = self.ctx.line_of(inner.range().start());
        self.clear_pending_call_args();
        self.push_edge(
            caller,
            CalleeRef::External {
                qualname: format!("eager-offload-call:{}:{inner_line}", kind.as_str()),
            },
            offload_range,
            CallKind::Call,
            false,
            Some(false),
        );
    }

    fn call_arg_returns_provably_non_callable(&self, arg: &Expr, inner: &ast::ExprCall) -> bool {
        if self.callee_is_unwrapped_async_def(&inner.func) {
            return true;
        }
        if self.expr_resolves_to_project_class_matching(&inner.func, |c| {
            class_node_is_non_callable_instance(c)
        }) {
            return true;
        }
        if let Some(defs) = self.resolve_callee_function_defs(&inner.func)
            && !defs.is_empty()
            && defs
                .iter()
                .all(|def| self.fn_def_returns_non_callable(*def))
        {
            return true;
        }
        let Some(ty) = arg.inferred_type(self.model) else {
            return false;
        };
        let env = self.model.program_environment();
        self.type_is_provably_non_callable(ty, &env)
    }

    fn fn_def_returns_non_callable(&self, def: Definition<'db>) -> bool {
        let DefinitionKind::Function(fdef) = def.kind(self.model.db()) else {
            return false;
        };
        let parsed =
            parsed_module(self.model.db(), def.python_file(self.model.db())).load(self.model.db());
        let node = fdef.node(&parsed);
        if !node
            .decorator_list
            .iter()
            .all(|d| decorator_preserves_coroutine(&d.expression))
        {
            return false;
        }
        if node.is_async || body_has_yield(&node.body) {
            return true;
        }
        let file = def.file(self.model.db());
        if let FilePath::System(sys) = file.path(self.model.db())
            && sys.extension() == Some("pyi")
        {
            return false;
        }
        fn_body_returns_only_non_callable_literals(&node.body)
    }

    fn type_is_provably_non_callable(
        &self,
        ty: Type<'db>,
        env: &ty_python_semantic::ProgramEnvironment<'db>,
    ) -> bool {
        if matches!(
            ty,
            Type::ClassLiteral(_)
                | Type::SubclassOf(_)
                | Type::GenericAlias(_)
                | Type::FunctionLiteral(_)
                | Type::BoundMethod(_)
                | Type::BoundSuper(_)
        ) {
            return false;
        }
        if ty.is_none(self.model.db()) {
            return true;
        }
        if matches!(ty, Type::LiteralValue(_) | Type::ModuleLiteral(_)) {
            return true;
        }
        if let Some(union) = ty.as_union() {
            let mut saw_arm = false;
            for elem in union.elements(self.model.db()).iter() {
                if matches!(elem, Type::Never) {
                    continue;
                }
                saw_arm = true;
                if !self.type_is_provably_non_callable(*elem, env) {
                    return false;
                }
            }
            return saw_arm;
        }
        let Type::NominalInstance(_) = ty else {
            return false;
        };
        let Some(TypeDefinition::StaticClass(def) | TypeDefinition::DynamicClass(def)) =
            ty.definition(self.model.db(), env)
        else {
            return false;
        };
        if self.class_def_matches(def, class_node_is_non_callable_instance) {
            return true;
        }
        let Some(name) = def.name(self.model.db()) else {
            return false;
        };
        let Some(module) = module_of_definition(self.model, def) else {
            return false;
        };
        let qual = format!("{module}.{name}");
        is_known_non_callable_instance_qual(&qual)
    }
}
