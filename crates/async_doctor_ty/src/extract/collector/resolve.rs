//! Callee resolution: project defs, externals, offloads, and follow targets.

use std::collections::HashSet;
use std::path::PathBuf;

use async_doctor_core::{CallEdge, CallKind, CalleeRef, FunctionId, OffloadKind};
use ruff_db::files::FilePath;
use ruff_db::parsed::parsed_module;
use ruff_python_ast::Expr;
use ruff_text_size::TextRange;
use ty_python_core::definition::DefinitionKind;
use ty_python_semantic::types::{Type, TypeDefinition};
use ty_python_semantic::{
    HasType, ImportAliasResolution, definitions_for_attribute, definitions_for_name,
};

use crate::sitepkgs::{FollowTarget, is_followable_site_packages_py};

fn is_under_followable_extra(path: &str, roots: &[std::path::PathBuf]) -> bool {
    let p = std::path::Path::new(path);
    roots.iter().any(|r| p.starts_with(r))
}

use super::CallCollector;
use crate::extract::ctx::DefKey;
use crate::extract::helpers::{
    RedisClientFamily, is_asyncio_redis_type, is_getattr_call, is_redis_commands_mixin,
    is_sync_redis_type, map_redis_mixin_to_canonical_sink, module_of_definition,
    normalize_asyncio_redis_type, qualname_from_definition,
};
use crate::extract::path::relative_display;

impl<'a, 'db> CallCollector<'a, 'db> {
    pub(crate) fn push_edge(
        &mut self,
        caller: FunctionId,
        callee: CalleeRef,
        range: TextRange,
        kind: CallKind,
        is_offload_wrapper: bool,
        callee_is_coroutine: Option<bool>,
    ) {
        // Bare generator call (`g = gen()`) must not propagate; only when consumed.
        if let CalleeRef::Project(id) = &callee
            && self.facts.function(*id).is_generator
            && !self.consuming_generator
            && kind != CallKind::GeneratorConsume
        {
            return;
        }

        // Project callees always know async-ness from FunctionFact.
        let callee_is_coroutine = match &callee {
            CalleeRef::Project(id) => Some(self.facts.function(*id).is_async),
            _ => callee_is_coroutine,
        };

        let key = match &callee {
            CalleeRef::Project(id) => format!("P{}", id.0),
            CalleeRef::External { qualname } => format!("E{qualname}"),
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
            positional_callables: std::mem::take(&mut self.pending_callables),
            keyword_callables: std::mem::take(&mut self.pending_keyword_callables),
            param_guard: self.active_param_guard,
            is_awaited: self.pending_is_awaited,
            callee_is_coroutine,
            is_eager_offload_arg: self.in_eager_offload_arg,
            context_from_caller: None,
        });
        self.pending_is_awaited = false;
    }

    /// Resolve whether `func` is an `async def` via ty definitions (MRO / `super`
    /// included). `None` when the definition cannot be recovered.
    pub(crate) fn resolve_callee_is_coroutine(&self, func: &Expr) -> Option<bool> {
        if let Some(fid) = self.resolve_project_function(func) {
            return Some(self.facts.function(fid).is_async);
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
            return Some(fdef.node(&parsed).is_async);
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
        Some(fdef.node(&parsed).is_async)
    }

    pub(crate) fn is_offload_callee(&self, func: &Expr) -> Option<OffloadKind> {
        // Ty-resolved only — do not treat bare `to_thread` / `run_in_threadpool`
        // attribute names as offload (a project function may share those names).
        let q = self.external_qualname(func)?;
        match q.as_str() {
            "asyncio.to_thread" | "asyncio.threads.to_thread" => Some(OffloadKind::AsyncioToThread),
            "anyio.to_thread.run_sync" => Some(OffloadKind::AnyioToThread),
            "starlette.concurrency.run_in_threadpool" => Some(OffloadKind::StarletteThreadpool),
            "concurrent.futures.Executor.submit"
            | "concurrent.futures.Executor.map"
            | "concurrent.futures.ThreadPoolExecutor.submit"
            | "concurrent.futures.ThreadPoolExecutor.map"
            | "concurrent.futures.ProcessPoolExecutor.submit"
            | "concurrent.futures.ProcessPoolExecutor.map" => Some(OffloadKind::ThreadPoolExecutor),
            other if other.ends_with(".run_in_executor") => Some(OffloadKind::RunInExecutor),
            _ => None,
        }
    }

    pub(crate) fn resolve_dunder(&self, expr: &Expr, dunder: &str) -> Option<CalleeRef> {
        let ty = expr.inferred_type(self.model)?;
        let env = self.model.program_environment();
        // Look up attribute via definitions_for_attribute by synthesizing isn't possible;
        // use member through display of Type::definition on attribute access via try_call-ish.
        // Fall back: resolve `{dunder}` on the expression's type through definitions_for_attribute
        // by building a temporary approach — use Type display + member isn't public enough.
        // Practical approach: if ty is NominalInstance / etc., use definitions_for_attribute
        // only works with ExprAttribute. So check inferred type of a virtual attribute via
        // `definitions_for_attribute` requires AST. Instead, use Type::definition on the
        // expression and then look for methods — too heavy.
        //
        // Cheap path: only resolve when `expr` type has a known definition class and we can
        // find `dunder` in project/stdlib via attribute completions? attribute_completions exists!
        let _ = (ty, env, dunder);
        // For M1: attempt type-based external name `{type}.{dunder}` when we can name the type.
        let type_name = self.type_instance_qualname(expr)?;
        Some(CalleeRef::External {
            qualname: format!("{type_name}.{dunder}"),
        })
    }

    pub(crate) fn type_instance_qualname(&self, expr: &Expr) -> Option<String> {
        // Cheap branch narrowing: isinstance / is not None on a local name.
        if let Expr::Name(n) = expr
            && let Some(fact) = self.narrowed_locals.get(n.id.as_str())
        {
            match fact {
                crate::extract::helpers::NarrowFact::IsInstance(ty) => {
                    return Some(ty.clone());
                }
                crate::extract::helpers::NarrowFact::NotNone => {
                    // Fall through to ty, then drop None-ish union members below.
                }
            }
        }
        let ty = expr.inferred_type(self.model)?;
        let env = self.model.program_environment();
        let mut name = self.type_to_instance_qualname(ty, &env)?;
        if let Expr::Name(n) = expr
            && matches!(
                self.narrowed_locals.get(n.id.as_str()),
                Some(crate::extract::helpers::NarrowFact::NotNone)
            )
            && (name.ends_with("None") || name.contains("None |") || name.contains("| None"))
        {
            // Defensive: if display still mentions None, prefer ty union filter.
            if let Some(union) = ty.as_union() {
                let env = self.model.program_environment();
                for elem in union.elements(self.model.db()).iter() {
                    if let Some(n) = self.type_to_instance_qualname(*elem, &env)
                        && !n.ends_with("None")
                        && n != "None"
                    {
                        name = n;
                        break;
                    }
                }
            }
        }
        Some(name)
    }

    /// Resolve a concrete instance qualname, including union receivers.
    ///
    /// For `Pipeline | ClusterPipeline` (all asyncio redis) → asyncio Pipeline.
    /// Mixed asyncio/sync redis → `None` (caller must not fall back to a sync
    /// mixin sink; prefer unresolved/candidate).
    pub(crate) fn type_to_instance_qualname(
        &self,
        ty: Type<'_>,
        env: &ty_python_semantic::ProgramEnvironment<'_>,
    ) -> Option<String> {
        if let Some(union) = ty.as_union() {
            let mut names: Vec<String> = Vec::new();
            for elem in union.elements(self.model.db()).iter() {
                // Skip trivial None/Never noise in Optional unions.
                if matches!(elem, Type::Never) {
                    continue;
                }
                if let Some(n) = self.type_to_instance_qualname(*elem, env) {
                    names.push(n);
                } else if elem.definition(self.model.db(), env).is_some() {
                    // Element has a definition we couldn't name — treat as opaque.
                    names.push(String::new());
                }
            }
            names.sort();
            names.dedup();
            names.retain(|n| !n.is_empty());
            if names.is_empty() {
                return None;
            }
            let all_asyncio = names.iter().all(|n| is_asyncio_redis_type(n));
            let any_asyncio = names.iter().any(|n| is_asyncio_redis_type(n));
            let any_sync_redis = names.iter().any(|n| is_sync_redis_type(n));
            if all_asyncio {
                // Prefer Pipeline when any member is a Pipeline.
                if names.iter().any(|n| n.contains("Pipeline")) {
                    return Some("redis.asyncio.client.Pipeline".to_string());
                }
                return Some("redis.asyncio.client.Redis".to_string());
            }
            if any_asyncio && any_sync_redis {
                // Mixed → do not pick a sync sink; signal via None so callers
                // avoid mixin→Redis normalization (emit unresolved instead).
                return None;
            }
            if names.len() == 1 {
                return Some(names.into_iter().next().unwrap());
            }
            // Homogeneous non-redis union: pick the first stable name.
            return names.into_iter().next();
        }

        let def = ty.definition(self.model.db(), env)?;
        match def {
            TypeDefinition::StaticClass(definition) | TypeDefinition::DynamicClass(definition) => {
                let name = definition.name(self.model.db())?;
                let module = module_of_definition(self.model, definition)?;
                Some(format!("{module}.{name}"))
            }
            TypeDefinition::Function(_) => None,
            TypeDefinition::Module(module) => {
                Some(module.name(self.model.db()).as_str().to_string())
            }
            _ => None,
        }
    }

    pub(crate) fn resolve_callee(&mut self, func: &Expr) -> Option<CalleeRef> {
        // Dynamic: getattr(...)  — never guess by name; emit unresolved candidate.
        if matches!(func, Expr::Call(c) if is_getattr_call(&c.func)) {
            return Some(CalleeRef::Unresolved {
                reason: "unresolved".into(),
            });
        }

        if let Some(fid) = self.resolve_project_function(func) {
            return Some(CalleeRef::Project(fid));
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
                let invokes = &mut self.facts.functions[fid.index()].invokes_params;
                if !invokes.contains(&index) {
                    invokes.push(index);
                }
            }
            return Some(CalleeRef::ParamInvoke { index });
        }

        if let Some(q) = self.external_qualname(func) {
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

    pub(crate) fn note_follow_target(&mut self, func: &Expr, qualname: &str) {
        if self.follow_targets.contains_key(qualname) {
            return;
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
            let file = def.file(self.model.db());
            let FilePath::System(sys) = file.path(self.model.db()) else {
                continue;
            };
            if !is_followable_site_packages_py(sys.as_str())
                && !is_under_followable_extra(sys.as_str(), self.followable_extra_roots)
            {
                continue;
            }
            let kind = def.kind(self.model.db());
            let DefinitionKind::Function(fdef) = kind else {
                continue;
            };
            let parsed = parsed_module(self.model.db(), def.python_file(self.model.db()))
                .load(self.model.db());
            let node = fdef.node(&parsed);
            let std_path = PathBuf::from(sys.as_str());
            let (pkg, ver, fp) = self
                .package_meta
                .meta_for_path(&std_path)
                .map(|(n, v, f)| (Some(n), Some(v), Some(f)))
                .unwrap_or((None, None, None));
            self.follow_targets.insert(
                qualname.to_string(),
                FollowTarget {
                    qualname: qualname.to_string(),
                    path: sys.to_path_buf(),
                    name_start: u32::from(node.name.range.start()),
                    package_name: pkg,
                    package_version: ver,
                    package_fingerprint: fp,
                },
            );
            return;
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

        for r in &resolved {
            if let Some(def) = r.definition()
                && let Some(fid) = self.def_from_definition(def)
            {
                return Some(fid);
            }
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
        let file = def.file(self.model.db());
        let path = file.path(self.model.db());
        let FilePath::System(sys) = path else {
            // Vendored / stub — not a project function.
            return None;
        };
        let rel = relative_display(self.ctx.db.project_root(), sys);
        let parsed =
            parsed_module(self.model.db(), def.python_file(self.model.db())).load(self.model.db());
        let kind = def.kind(self.model.db());
        let DefinitionKind::Function(fdef) = kind else {
            return None;
        };
        let node = fdef.node(&parsed);
        let key = DefKey {
            file: rel,
            name_start: u32::from(node.name.range.start()),
        };
        self.def_map.get(&key).copied()
    }

    pub(crate) fn external_qualname(&self, func: &Expr) -> Option<String> {
        // For attributes, ALWAYS prefer `{receiver_type}.{attr}` over the
        // defining class. Mixin-inherited methods (e.g. redis CoreCommands.set
        // on a Pipeline instance) must key off the *receiver* so Pipeline
        // buffering is not mistaken for Redis IO.
        if let Expr::Attribute(attr) = func {
            // Prefer bindings from `async with r.pipeline() as pipe`.
            if let Expr::Name(n) = attr.value.as_ref()
                && let Some(recv) = self.async_with_receivers.get(n.id.as_str())
            {
                return Some(format!("{recv}.{}", attr.attr.as_str()));
            }
            // Union receivers (Pipeline | ClusterPipeline, Redis | RedisCluster).
            if let Some(ty) = attr.value.inferred_type(self.model) {
                let env = self.model.program_environment();
                if ty.as_union().is_some() {
                    match self.type_to_instance_qualname(ty, &env) {
                        Some(recv) => {
                            let recv = if is_asyncio_redis_type(&recv) {
                                normalize_asyncio_redis_type(&recv)
                            } else {
                                recv
                            };
                            return Some(format!("{recv}.{}", attr.attr.as_str()));
                        }
                        None => {
                            // Mixed asyncio|sync (or unnameable union): never fall
                            // through to CoreCommands → sync Redis.
                            if self.expr_looks_like_asyncio_redis(&attr.value) {
                                return Some(format!(
                                    "redis.asyncio.client.Pipeline.{}",
                                    attr.attr.as_str()
                                ));
                            }
                            return None;
                        }
                    }
                }
            }
            if let Some(recv) = self.type_instance_qualname(&attr.value) {
                let recv = if is_asyncio_redis_type(&recv) {
                    normalize_asyncio_redis_type(&recv)
                } else {
                    recv
                };
                // Known redis receivers (Redis / Pipeline / asyncio): prefer
                // receiver over mixin defining class.
                if is_asyncio_redis_type(&recv) || is_sync_redis_type(&recv) {
                    return Some(format!("{recv}.{}", attr.attr.as_str()));
                }
                // Project subclass / `super()` inside an override: map through
                // the MRO defining class so sync-sink status is inherited;
                // async defs stay on the asyncio key.
                if let Some(mapped) = self.attribute_redis_mixin_sink(attr) {
                    return Some(mapped);
                }
                return Some(format!("{recv}.{}", attr.attr.as_str()));
            }
            // dist-packages / weak typing: still recognize asyncio redis receivers.
            if self.expr_looks_like_asyncio_redis(&attr.value) {
                return Some(format!("redis.asyncio.client.Redis.{}", attr.attr.as_str()));
            }
            // Do not map bare CoreCommands without a typed non-redis receiver:
            // untyped Pipeline shares that mixin and must stay non-sink.
        }

        // Prefer definitions_for_* → file_to_module + name + enclosing class.
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
            if let Some(def) = r.definition()
                && let Some(q) = qualname_from_definition(self.model, def)
            {
                // Bare redis mixin defs (no Attribute receiver context) are not
                // sync-IO evidence — Pipeline buffering shares those mixins.
                if is_redis_commands_mixin(&q) {
                    continue;
                }
                return Some(q);
            }
        }

        let ty = func.inferred_type(self.model)?;
        let env = self.model.program_environment();
        match ty.definition(self.model.db(), &env)? {
            TypeDefinition::Function(def) => {
                let q = qualname_from_definition(self.model, def)?;
                if is_redis_commands_mixin(&q) {
                    return None;
                }
                Some(q)
            }
            TypeDefinition::Module(module) => {
                Some(module.name(self.model.db()).as_str().to_string())
            }
            TypeDefinition::StaticClass(def) | TypeDefinition::DynamicClass(def) => {
                let name = def.name(self.model.db())?;
                let module = module_of_definition(self.model, def)?;
                Some(format!("{module}.{name}"))
            }
            _ => None,
        }
    }

    /// When the receiver is a project subclass of redis, map the MRO defining
    /// class (commands mixin) to the canonical `redis.Redis.*` /
    /// `redis.asyncio.Redis.*` sink key — family from the *receiver* MRO.
    fn attribute_redis_mixin_sink(&self, attr: &ruff_python_ast::ExprAttribute) -> Option<String> {
        let family = self.receiver_redis_family(&attr.value)?;
        let try_def = |def: ty_python_core::definition::Definition<'db>| -> Option<String> {
            let q = qualname_from_definition(self.model, def)?;
            map_redis_mixin_to_canonical_sink(&q, family)
        };

        let resolved = definitions_for_attribute(self.model, attr);
        for r in &resolved {
            if let Some(def) = r.definition()
                && let Some(mapped) = try_def(def)
            {
                return Some(mapped);
            }
        }
        // BoundMethod / FunctionLiteral on the attribute expression itself.
        let ty = attr.inferred_type(self.model)?;
        let env = self.model.program_environment();
        match ty.definition(self.model.db(), &env)? {
            TypeDefinition::Function(def) => try_def(def),
            _ => None,
        }
    }

    /// Sync vs asyncio redis for a receiver expression, via the type MRO
    /// (or the enclosing class when the receiver is `super()`).
    fn receiver_redis_family(&self, expr: &Expr) -> Option<RedisClientFamily> {
        if self.expr_is_super(expr) {
            return self.enclosing_class_redis_family();
        }
        let ty = expr.inferred_type(self.model)?;
        self.type_redis_family(ty, &mut HashSet::new())
    }

    fn expr_is_super(&self, expr: &Expr) -> bool {
        match expr {
            Expr::Name(n) => n.id.as_str() == "super",
            Expr::Call(call) => match call.func.as_ref() {
                Expr::Name(n) => n.id.as_str() == "super",
                Expr::Attribute(attr) => {
                    attr.attr.as_str() == "super"
                        && matches!(attr.value.as_ref(), Expr::Name(n) if n.id.as_str() == "builtins")
                }
                _ => false,
            },
            _ => self
                .type_instance_qualname(expr)
                .is_some_and(|q| q == "builtins.super" || q.ends_with(".super")),
        }
    }

    /// Walk the enclosing class's bases (for `super().get()` inside an override).
    fn enclosing_class_redis_family(&self) -> Option<RedisClientFamily> {
        let class_name = self.class_stack.last()?;
        let parsed = parsed_module(
            self.model.db(),
            self.ctx.program_file.python_file(self.model.db()),
        )
        .load(self.model.db());
        let class_def = find_class_def(parsed.suite(), self.class_stack.as_slice())?;
        debug_assert_eq!(class_def.name.as_str(), class_name.as_str());
        self.class_def_redis_family(class_def, &mut HashSet::new())
    }

    fn type_redis_family(
        &self,
        ty: Type<'db>,
        visited: &mut HashSet<String>,
    ) -> Option<RedisClientFamily> {
        let env = self.model.program_environment();
        if let Some(q) = self.type_to_instance_qualname(ty, &env) {
            if is_asyncio_redis_type(&q) {
                return Some(RedisClientFamily::Async);
            }
            if is_sync_redis_type(&q) {
                return Some(RedisClientFamily::Sync);
            }
            if !visited.insert(q) {
                return None;
            }
        }
        let typedef = ty.definition(self.model.db(), &env)?;
        let def = match typedef {
            TypeDefinition::StaticClass(d) | TypeDefinition::DynamicClass(d) => d,
            _ => return None,
        };
        // Module path of the class itself (site-packages redis.asyncio.*).
        if let Some(module) = module_of_definition(self.model, def) {
            if is_asyncio_redis_type(&module) {
                return Some(RedisClientFamily::Async);
            }
            if module.starts_with("redis.") && !module.contains("asyncio") {
                // redis.client.Redis / redis.connection — sync family.
                let name = def.name(self.model.db()).unwrap_or_default();
                if name.contains("Redis")
                    || name.contains("Pipeline")
                    || name.contains("StrictRedis")
                {
                    return Some(RedisClientFamily::Sync);
                }
            }
        }
        let kind = def.kind(self.model.db());
        let DefinitionKind::Class(class_ref) = kind else {
            return None;
        };
        let parsed =
            parsed_module(self.model.db(), def.python_file(self.model.db())).load(self.model.db());
        let class_def = class_ref.node(&parsed);
        self.class_def_redis_family(class_def, visited)
    }

    fn class_def_redis_family(
        &self,
        class_def: &ruff_python_ast::StmtClassDef,
        visited: &mut HashSet<String>,
    ) -> Option<RedisClientFamily> {
        // Prefer asyncio if any base is asyncio redis; sync if any sync redis.
        // Mixed bases should not happen for redis clients; asyncio wins if both
        // somehow appear (never map an asyncio subclass to a sync sink).
        let mut saw_sync = false;
        for base in class_def.bases() {
            if let Some(family) = self.base_expr_redis_family(base, visited) {
                match family {
                    RedisClientFamily::Async => return Some(RedisClientFamily::Async),
                    RedisClientFamily::Sync => saw_sync = true,
                }
            }
        }
        saw_sync.then_some(RedisClientFamily::Sync)
    }

    fn base_expr_redis_family(
        &self,
        base: &Expr,
        visited: &mut HashSet<String>,
    ) -> Option<RedisClientFamily> {
        if let Some(q) = self.type_instance_qualname(base) {
            if is_asyncio_redis_type(&q) {
                return Some(RedisClientFamily::Async);
            }
            if is_sync_redis_type(&q) {
                return Some(RedisClientFamily::Sync);
            }
        }
        // Import / attribute bases: `redis.asyncio.Redis`, bare `Redis` from
        // `from redis.asyncio import Redis`.
        if let Some(q) = self.external_qualname_for_base(base) {
            if is_asyncio_redis_type(&q) {
                return Some(RedisClientFamily::Async);
            }
            if is_sync_redis_type(&q) {
                return Some(RedisClientFamily::Sync);
            }
        }
        if self.expr_looks_like_asyncio_redis(base) {
            return Some(RedisClientFamily::Async);
        }
        let ty = base.inferred_type(self.model)?;
        // Class object (not instance): definition is the class itself.
        self.type_redis_family(ty, visited)
    }

    /// Qualname of a base expression without treating it as a call target.
    fn external_qualname_for_base(&self, base: &Expr) -> Option<String> {
        let resolved = match base {
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
            if let Some(def) = r.definition() {
                if let FilePath::System(sys) = def.file(self.model.db()).path(self.model.db()) {
                    let s = sys.as_str().replace('\\', "/");
                    if s.contains("/redis/asyncio/") || s.contains("/redis/asyncio.py") {
                        let name = def.name(self.model.db())?;
                        return Some(format!("redis.asyncio.{name}"));
                    }
                }
                if let Some(q) = qualname_from_definition(self.model, def) {
                    return Some(q);
                }
            }
        }
        None
    }
}

/// Find a class definition matching `class_stack` (outermost → innermost).
fn find_class_def<'a>(
    body: &'a [ruff_python_ast::Stmt],
    class_stack: &[String],
) -> Option<&'a ruff_python_ast::StmtClassDef> {
    if class_stack.is_empty() {
        return None;
    }
    let (head, rest) = class_stack.split_first()?;
    for stmt in body {
        let ruff_python_ast::Stmt::ClassDef(class) = stmt else {
            continue;
        };
        if class.name.as_str() != head.as_str() {
            continue;
        }
        if rest.is_empty() {
            return Some(class);
        }
        return find_class_def(&class.body, rest);
    }
    None
}
