//! Receiver instance typing and external callee qualname synthesis.

use async_doctor_rules::aws_op_method_name;
use ruff_python_ast::Expr;
use ty_python_semantic::types::{Type, TypeDefinition};
use ty_python_semantic::{
    HasType, ImportAliasResolution, definitions_for_attribute, definitions_for_name,
};

use super::CallCollector;
use crate::extract::helpers::{
    is_asyncio_redis_type, is_redis_commands_mixin, is_sync_redis_type, module_of_definition,
    normalize_asyncio_redis_type, qualname_from_definition,
};

impl<'a, 'db> CallCollector<'a, 'db> {
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
            // Only collapse a multi-arm non-redis union when every arm comes
            // from the same root package and agrees on async-ness (e.g.
            // `PosixPath | WindowsPath` or `Redis | RedisCluster`). Mixed
            // sync/async or cross-package unions (`Client | AsyncClient`) must
            // not guess a sync arm.
            let is_async_arm =
                |s: &str| s.contains("Async") || s.contains("aio") || s.contains("asyncio");
            let first_root = names[0].split('.').next().unwrap_or("");
            let first_async = is_async_arm(&names[0]);
            let homogeneous = names.iter().all(|n| {
                n.split('.').next().unwrap_or("") == first_root && is_async_arm(n) == first_async
            });
            if homogeneous {
                return names.into_iter().next();
            }
            return None;
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

    pub(crate) fn external_qualname(&mut self, func: &Expr) -> Option<String> {
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
            // Explicit `isinstance(x, Cls)` branch narrowing overrides wider
            // union inference on `x`.
            if let Expr::Name(n) = attr.value.as_ref()
                && let Some(crate::extract::helpers::NarrowFact::IsInstance(recv)) =
                    self.narrowed_locals.get(n.id.as_str()).cloned()
            {
                let recv = if is_asyncio_redis_type(&recv) {
                    normalize_asyncio_redis_type(&recv)
                } else {
                    recv
                };
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
            // boto3 clients are dynamically typed (stubs often return `object`).
            // Prefer syntactic receivers before weak typed `{object}.get_object`.
            // Never synthesize sync botocore ops for aioboto3/aiobotocore receivers
            // (incl. `async with session.client(...) as x` / awaited calls on them).
            if aws_op_method_name(attr.attr.as_str())
                && !self.expr_is_aio_boto_receiver(&attr.value)
                && !(self.pending_is_awaited && self.expr_binds_aio_boto(&attr.value))
                && self.expr_is_boto_factory_or_receiver(&attr.value)
            {
                return Some(format!("botocore.client.BaseClient.{}", attr.attr.as_str()));
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

        if let Some(q) = self.resolve_uninstalled_resource_qualname(func) {
            return Some(q);
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
}
