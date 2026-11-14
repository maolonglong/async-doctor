//! Callee resolution: project defs, externals, offloads, and follow targets.

mod external;
mod follow_target;

use std::sync::OnceLock;

use async_doctor_core::{CallEdge, CallKind, CalleeRef, FunctionId, OffloadKind, SinkTable};
use async_doctor_rules::BuiltinSinkTable;
use ruff_db::files::FilePath;
use ruff_db::parsed::parsed_module;
use ruff_python_ast::{self as ast, Expr};
use ruff_text_size::{Ranged, TextRange};
use ty_python_core::ast_ids::HasScopedUseId;
use ty_python_core::definition::{Definition, DefinitionKind};
use ty_python_core::scope::NodeWithScopeKind;
use ty_python_core::use_def_map;
use ty_python_semantic::types::{Type, TypeDefinition};
use ty_python_semantic::{
    HasType, ImportAliasResolution, definitions_for_attribute, definitions_for_name,
};

fn is_get_event_loop_qual(qualname: &str) -> bool {
    matches!(
        qualname,
        "asyncio.get_running_loop"
            | "asyncio.get_event_loop"
            | "asyncio.events.get_running_loop"
            | "asyncio.events.get_event_loop"
            | "asyncio.runners.get_running_loop"
    ) || qualname.ends_with(".get_running_loop")
        || qualname.ends_with(".get_event_loop")
}

fn is_nested_event_loop_qual_ty(qualname: &str) -> bool {
    matches!(
        qualname,
        "asyncio.run"
            | "asyncio.runners.run"
            | "asyncio.events.AbstractEventLoop.run_until_complete"
            | "asyncio.events.AbstractEventLoop.run_forever"
            | "asyncio.base_events.BaseEventLoop.run_until_complete"
            | "asyncio.base_events.BaseEventLoop.run_forever"
            | "asyncio.Runner.run"
            | "asyncio.runners.Runner.run"
            | "anyio.run"
            | "trio.run"
    ) || qualname.ends_with(".run_until_complete")
        || qualname.ends_with(".run_forever")
        || (qualname.starts_with("asyncio.") && qualname.ends_with(".Runner.run"))
}

/// Occurrence counter key for an external callee.
///
/// Finding IDs hash the *canonical sink* qualname (after family aliases), not the
/// original callee. Count occurrence on that same key so `boto3.client` and
/// `boto3.Session.client` (both → sink `boto3.client`) or `asyncio.run` +
/// `anyio.run` (both → `nested-event-loop`) get distinct ordinals.
fn external_occurrence_key(qualname: &str) -> String {
    if is_nested_event_loop_qual_ty(qualname) {
        return "Enested-event-loop".to_string();
    }
    if qualname.starts_with("eager-offload-call:") {
        return "Eeager-offload-call".to_string();
    }
    if qualname.starts_with("ephemeral-async-client:") {
        return "Eephemeral-async-client".to_string();
    }
    if qualname.starts_with("shared-async-session:") {
        return "Eshared-async-session".to_string();
    }
    if qualname.starts_with("offload-stateful-session:") {
        return "Eoffload-stateful-session".to_string();
    }
    if qualname.starts_with("useless-local-async-lock:") {
        return "Euseless-local-async-lock".to_string();
    }
    if qualname.starts_with("swallowed-cancelled-error:") {
        return "Eswallowed-cancelled-error".to_string();
    }
    static SINKS: OnceLock<BuiltinSinkTable> = OnceLock::new();
    let sinks = SINKS.get_or_init(BuiltinSinkTable::new);
    let key = sinks
        .lookup(qualname)
        .map(|h| h.sink_qualname)
        .unwrap_or(qualname);
    format!("E{key}")
}

use super::CallCollector;
use crate::extract::constants::body_has_yield;
use crate::extract::ctx::DefKey;
use crate::extract::helpers::is_getattr_call;
use crate::extract::path::relative_display;

impl<'a, 'db> CallCollector<'a, 'db> {
    /// IDE name navigation includes all scope bindings, not just the bindings
    /// reaching this expression. Execution proofs need the actual use-def map.
    pub(super) fn local_definition_at_use(&self, name: &ast::ExprName) -> Option<Definition<'db>> {
        let db = self.model.db();
        let scope = self
            .model
            .scope(name.into())?
            .to_scope_id(db, self.model.program_file());
        let use_def = use_def_map(db, scope);
        let mut bindings =
            use_def.bindings_at_use(name.scoped_use_id(db, self.model.program_file()));
        let def = bindings.next()?.binding.definition()?;
        if bindings.next().is_some() || def.file(db) != self.model.file() {
            return None;
        }
        let parsed = parsed_module(db, def.python_file(db)).load(db);
        let start = match def.scope(db).node(db) {
            NodeWithScopeKind::Function(f) => f.node(&parsed).name.range.start(),
            NodeWithScopeKind::Lambda(l) => l.node(&parsed).range.start(),
            _ => return None,
        };
        let key = DefKey {
            file: self.ctx.rel.clone(),
            name_start: u32::from(start),
        };
        (self.def_get(&key) == self.current_fn).then_some(def)
    }

    pub(crate) fn push_edge(
        &mut self,
        caller: FunctionId,
        callee: CalleeRef,
        range: TextRange,
        kind: CallKind,
        is_offload_wrapper: bool,
        callee_is_coroutine: Option<bool>,
    ) {
        // Bare generator call (`g = gen()`) must not propagate; only when consumed
        // or entered as a context manager (`with cm():` / `async with acm():`).
        if let CalleeRef::Project(id) = &callee
            && self.function(*id).is_generator
            && !self.consuming_generator
            && !matches!(
                kind,
                CallKind::GeneratorConsume | CallKind::ForIter | CallKind::WithEnter
            )
        {
            return;
        }

        // Coroutine and async-generator definitions have different call results.
        let callee_is_coroutine = match &callee {
            CalleeRef::Project(id) => Some(self.function(*id).is_coroutine()),
            _ => callee_is_coroutine,
        };

        let key = match &callee {
            CalleeRef::Project(id) => format!("P{}", id.0),
            CalleeRef::External { qualname } => {
                if is_get_event_loop_qual(qualname) {
                    self.try_saw_get_loop = true;
                }
                external_occurrence_key(qualname)
            }
            CalleeRef::Unresolved { reason } => format!("U{reason}"),
            CalleeRef::Offload(k) => format!("O{}", k.as_str()),
            CalleeRef::ParamInvoke { index } => format!("I{index}"),
        };
        let occ = self.occurrence_counter.entry((caller, key)).or_insert(0);
        let occurrence = *occ;
        *occ += 1;

        self.facts.edges.push(CallEdge {
            caller,
            callee,
            file: self.ctx.rel.clone(),
            line: self.ctx.line_of(range.start()),
            text: self.ctx.snippet(range),
            kind,
            conditions: self.conditions_now(),
            occurrence,
            is_offload_wrapper,
            positional_literals: std::mem::take(&mut self.pending_literals),
            keyword_literals: std::mem::take(&mut self.pending_keyword_literals),
            positional_callables: std::mem::take(&mut self.pending_callables),
            positional_externals: std::mem::take(&mut self.pending_externals),
            positional_constructed_receivers: std::mem::take(
                &mut self.pending_constructed_receivers,
            ),
            keyword_callables: std::mem::take(&mut self.pending_keyword_callables),
            param_guards: self.active_param_guards.clone(),
            is_awaited: self.pending_is_awaited,
            callee_is_coroutine,
            is_eager_offload_arg: self.in_eager_offload_arg,
            is_bound_call: self.pending_is_bound_call,
            context_from_caller: None,
            context_from_edge: None,
            constructed_receiver: self.pending_constructed_receiver.take(),
        });
        self.pending_is_awaited = false;
        self.pending_is_bound_call = false;
    }

    /// True when `func` binds parameter 0 (`self` or `cls`) implicitly at the call site.
    pub(crate) fn expr_is_bound_method_call(&self, func: &Expr, fid: FunctionId) -> bool {
        if let Some(ty) = func.inferred_type(self.model) {
            if matches!(ty, Type::BoundMethod(_) | Type::BoundSuper(_)) {
                return true;
            }
            if matches!(ty, Type::FunctionLiteral(_)) {
                return false;
            }
        }
        let Expr::Attribute(attr) = func else {
            return false;
        };
        if self.expr_is_super_call(&attr.value) {
            return matches!(
                self.function(fid).param_names.first().map(String::as_str),
                Some("self" | "cls")
            );
        }
        if let Some(ty) = attr.value.inferred_type(self.model) {
            if matches!(ty, Type::ClassLiteral(_)) {
                return self.function(fid).param_names.first().map(String::as_str) == Some("cls");
            }
            let env = self.model.program_environment();
            if matches!(
                ty.definition(self.model.db(), &env),
                Some(TypeDefinition::Module(_))
            ) {
                return false;
            }
        }
        matches!(
            self.function(fid).param_names.first().map(String::as_str),
            Some("self" | "cls")
        )
    }

    /// True when a non-project call expression binds parameter 0 (`self`/`cls`) implicitly.
    pub(crate) fn expr_is_external_bound_call(&self, func: &Expr) -> bool {
        if let Some(ty) = func.inferred_type(self.model) {
            if matches!(ty, Type::BoundMethod(_) | Type::BoundSuper(_)) {
                return true;
            }
            if matches!(ty, Type::FunctionLiteral(_)) {
                return false;
            }
        }
        let Expr::Attribute(attr) = func else {
            return false;
        };
        if self.expr_is_super_call(&attr.value) {
            return true;
        }
        if let Some(ty) = attr.value.inferred_type(self.model) {
            if matches!(ty, Type::ClassLiteral(_)) {
                return false;
            }
            let env = self.model.program_environment();
            if matches!(
                ty.definition(self.model.db(), &env),
                Some(TypeDefinition::Module(_))
            ) {
                return false;
            }
        }
        true
    }

    /// Resolve whether `func` is a coroutine function, excluding async generators
    /// (MRO / `super` included). `None` when the definition cannot be recovered.
    pub(crate) fn resolve_callee_is_coroutine(&self, func: &Expr) -> Option<bool> {
        if let Some(fid) = self.resolve_project_function(func) {
            return Some(self.function(fid).is_coroutine());
        }
        if !self.resolve_project_constructors(func).is_empty() {
            return Some(false);
        }
        let resolved = match func {
            Expr::Name(name) => definitions_for_name(
                self.model,
                name.id.as_str(),
                name.into(),
                ImportAliasResolution::ResolveAliases,
            ),
            Expr::Attribute(attr) => definitions_for_attribute(self.model, attr),
            _ => Vec::new(),
        };
        for r in &resolved {
            let Some(def) = r.definition() else {
                continue;
            };
            let kind = def.kind(self.model.db());
            let DefinitionKind::Function(fdef) = kind else {
                continue;
            };
            let parsed = parsed_module(self.model.db(), def.python_file(self.model.db()))
                .load(self.model.db());
            let node = fdef.node(&parsed);
            return Some(node.is_async && !body_has_yield(&node.body));
        }
        // Type-based FunctionLiteral / BoundMethod → definition.
        let ty = func.inferred_type(self.model)?;
        let env = self.model.program_environment();
        let typedef = ty.definition(self.model.db(), &env)?;
        let TypeDefinition::Function(def) = typedef else {
            return None;
        };
        let kind = def.kind(self.model.db());
        let DefinitionKind::Function(fdef) = kind else {
            return None;
        };
        let parsed =
            parsed_module(self.model.db(), def.python_file(self.model.db())).load(self.model.db());
        let node = fdef.node(&parsed);
        Some(node.is_async && !body_has_yield(&node.body))
    }

    pub(crate) fn is_offload_callee(&mut self, func: &Expr) -> Option<OffloadKind> {
        // Ty-resolved only — do not treat bare `to_thread` / `run_in_threadpool`
        // attribute names as offload (a project function may share those names).
        let q = self.external_qualname(func)?;
        Self::offload_kind_from_qualname(&q)
    }

    pub(crate) fn offload_kind_from_qualname(q: &str) -> Option<OffloadKind> {
        match q {
            "asyncio.to_thread" | "asyncio.threads.to_thread" => Some(OffloadKind::AsyncioToThread),
            "anyio.to_thread.run_sync" => Some(OffloadKind::AnyioToThread),
            "starlette.concurrency.run_in_threadpool" => Some(OffloadKind::StarletteThreadpool),
            "asgiref.sync.sync_to_async" => Some(OffloadKind::AsgirefSyncToAsync),
            "channels.db.database_sync_to_async" => Some(OffloadKind::ChannelsDbSyncToAsync),
            // `Executor.map` blocks on consumption — handled as a sink, not an
            // offload cut (unlike `submit`, which returns a Future immediately).
            "concurrent.futures.Executor.submit"
            | "concurrent.futures.ThreadPoolExecutor.submit"
            | "concurrent.futures.ProcessPoolExecutor.submit" => {
                Some(OffloadKind::ThreadPoolExecutor)
            }
            other if other.ends_with(".run_in_executor") => Some(OffloadKind::RunInExecutor),
            _ => None,
        }
    }

    pub(crate) fn resolve_dunder(&self, expr: &Expr, dunder: &str) -> Option<CalleeRef> {
        let ty = expr.inferred_type(self.model)?;
        if ty.as_union().is_none()
            && !matches!(
                ty,
                Type::ClassLiteral(_) | Type::SubclassOf(_) | Type::GenericAlias(_)
            )
        {
            // The IDE lookup uses ty's MRO and only infers the real receiver;
            // the synthetic attribute itself need not be in the parsed tree.
            let attr = ast::ExprAttribute {
                node_index: ast::AtomicNodeIndex::NONE,
                range: expr.range(),
                value: Box::new(expr.clone()),
                attr: ast::Identifier::new(dunder, expr.range()),
                ctx: ast::ExprContext::Load,
            };
            let mut targets = Vec::new();
            for resolved in definitions_for_attribute(self.model, &attr) {
                let Some(def) = resolved.definition() else {
                    targets.clear();
                    break;
                };
                let DefinitionKind::Function(fdef) = def.kind(self.model.db()) else {
                    // Instance assignments and dynamic attribute hooks are not
                    // implicit special-method calls on the receiver's type.
                    targets.clear();
                    break;
                };
                let parsed = parsed_module(self.model.db(), def.python_file(self.model.db()))
                    .load(self.model.db());
                let Some(fid) = self.def_from_definition(def).filter(|&fid| {
                    fdef.node(&parsed).name.as_str() == dunder
                        && !self.function(fid).is_installed_third_party()
                        && (!self.function(fid).is_generator || dunder == "__iter__")
                }) else {
                    targets.clear();
                    break;
                };
                if !targets.contains(&fid) {
                    targets.push(fid);
                }
            }
            if let [fid] = targets.as_slice() {
                return Some(CalleeRef::Project(*fid));
            }
        }
        // Synthesize type-based external name `{type}.{dunder}` when the receiver type is known.
        let type_name = self.type_instance_qualname(expr)?;
        Some(CalleeRef::External {
            qualname: format!("{type_name}.{dunder}"),
        })
    }

    pub(crate) fn resolve_non_project_callee(
        &mut self,
        func: &Expr,
        ext_qual: Option<String>,
    ) -> Option<CalleeRef> {
        // Dynamic: getattr(...)  — never guess by name; emit unresolved candidate.
        if matches!(func, Expr::Call(c) if is_getattr_call(&c.func)) {
            return Some(CalleeRef::Unresolved {
                reason: "unresolved".into(),
            });
        }

        // Higher-order: calling a positional parameter by name.
        if let Expr::Name(name) = func
            && let Some(idx) = self
                .current_param_names
                .iter()
                .position(|p| p == name.id.as_str())
        {
            let index = idx as u32;
            if let Some(fid) = self.current_fn {
                self.record_param_invoke(fid, index);
            }
            return Some(CalleeRef::ParamInvoke { index });
        }

        if let Some(q) = ext_qual {
            self.note_follow_target(func, &q);
            return Some(CalleeRef::External { qualname: q });
        }

        // Clearly dynamic / registry-style callables → candidates.
        // Plain unresolved Name/Attribute: omit the edge (cannot prove a sink).
        // Failed *imports* are recorded separately (see `note_import*`) so they
        // are not silently dropped without exploding per-call candidate counts.
        match func {
            Expr::Subscript(_) | Expr::Call(_) | Expr::BoolOp(_) | Expr::If(_) => {
                Some(CalleeRef::Unresolved {
                    reason: "unresolved".into(),
                })
            }
            _ => None,
        }
    }

    pub(crate) fn resolve_project_function(&self, func: &Expr) -> Option<FunctionId> {
        let resolved = match func {
            Expr::Name(name) => definitions_for_name(
                self.model,
                name.id.as_str(),
                name.into(),
                ImportAliasResolution::ResolveAliases,
            ),
            Expr::Attribute(attr) => definitions_for_attribute(self.model, attr),
            _ => {
                // Fall through to type-based definition.
                Vec::new()
            }
        };

        let mut fids: Vec<FunctionId> = Vec::new();
        for r in &resolved {
            if let Some(def) = r.definition()
                && let Some(fid) = self.def_from_definition(def)
                && !fids.contains(&fid)
            {
                fids.push(fid);
            }
        }
        if let Expr::Attribute(attr) = func
            && let Expr::Name(n) = attr.value.as_ref()
            && let Some(crate::extract::helpers::NarrowFact::IsInstance(narrowed)) =
                self.narrowed_locals.get(n.id.as_str())
        {
            let expected = format!("{narrowed}.{}", attr.attr.as_str());
            let short = narrowed.rsplit('.').next().unwrap_or(narrowed.as_str());
            let expected_short = format!("{short}.{}", attr.attr.as_str());
            for &fid in &fids {
                let f = self.function(fid);
                let full_dot = if f.module.is_empty() {
                    f.qualname.clone()
                } else {
                    format!("{}.{}", f.module, f.qualname)
                };
                if full_dot == expected || f.qualname == expected_short {
                    return Some(fid);
                }
            }
            if let Some(fid) = self.qual_get(&(self.ctx.module.clone(), expected_short)) {
                return Some(fid);
            }
        }
        if fids.len() == 1 {
            return Some(fids[0]);
        }
        if fids.len() > 1 {
            // Multiple distinct project targets on an unnarrowed union
            // (e.g. `SyncClient | AsyncClient`): do not guess one arm.
            return None;
        }

        if let Expr::Attribute(attr) = func
            && let Some(fid) = self.resolve_inherited_or_super_method(attr)
        {
            return Some(fid);
        }

        // Type-based: FunctionLiteral / BoundMethod → definition.
        let ty = func.inferred_type(self.model)?;
        let env = self.model.program_environment();
        let typedef = ty.definition(self.model.db(), &env)?;
        if let TypeDefinition::Function(def) = typedef {
            return self.def_from_definition(def);
        }
        None
    }

    pub(crate) fn def_from_definition(
        &self,
        def: ty_python_core::definition::Definition<'db>,
    ) -> Option<FunctionId> {
        let db = self.model.db();
        let DefinitionKind::Function(fdef) = def.kind(db) else {
            return None;
        };
        let file = def.file(db);
        let path = file.path(db);
        let FilePath::System(sys) = path else {
            // Vendored / stub — not a project function.
            return None;
        };
        let rel = relative_display(self.ctx.db.project_root(), sys);
        let parsed = parsed_module(db, def.python_file(db)).load(db);
        let node = fdef.node(&parsed);
        let key = DefKey {
            file: rel,
            name_start: u32::from(node.name.range.start()),
        };
        self.def_get(&key)
    }
}
