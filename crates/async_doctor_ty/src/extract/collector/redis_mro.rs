//! Redis receiver MRO / mixin sink mapping for attribute calls.
//!
//! Kept separate from general callee resolution so `resolve.rs` stays under
//! the source-size ceiling; CallCollector methods live in the same module tree.

use std::collections::HashSet;

use ruff_db::files::FilePath;
use ruff_db::parsed::parsed_module;
use ruff_python_ast::Expr;
use ty_python_core::definition::DefinitionKind;
use ty_python_semantic::types::{Type, TypeDefinition};
use ty_python_semantic::{
    HasType, ImportAliasResolution, definitions_for_attribute, definitions_for_name,
};

use super::CallCollector;
use crate::extract::helpers::{
    RedisClientFamily, is_asyncio_redis_type, is_redis_commands_core_method, is_sync_redis_type,
    map_redis_mixin_to_canonical_sink, module_of_definition, qualname_from_definition,
};

impl<'a, 'db> CallCollector<'a, 'db> {
    /// When the receiver is a project subclass of redis, map the MRO defining
    /// class (commands mixin) to the canonical `redis.Redis.*` /
    /// `redis.asyncio.Redis.*` sink key — family from the *receiver* MRO.
    ///
    /// Resolve the attribute first and only walk receiver MRO when the defining
    /// class is a redis commands mixin (avoids ~base-expression inference on
    /// every typed attribute call).
    pub(crate) fn attribute_redis_mixin_sink(
        &mut self,
        attr: &ruff_python_ast::ExprAttribute,
    ) -> Option<String> {
        let mut mixin_quals: Vec<String> = Vec::new();
        let resolved = definitions_for_attribute(self.model, attr);
        for r in &resolved {
            if let Some(def) = r.definition()
                && let Some(q) = qualname_from_definition(self.model, def)
                && is_redis_commands_core_method(&q)
            {
                mixin_quals.push(q);
            }
        }
        if mixin_quals.is_empty() {
            // BoundMethod / FunctionLiteral on the attribute expression itself.
            let ty = attr.inferred_type(self.model)?;
            let env = self.model.program_environment();
            if let TypeDefinition::Function(def) = ty.definition(self.model.db(), &env)? {
                let q = qualname_from_definition(self.model, def)?;
                if is_redis_commands_core_method(&q) {
                    mixin_quals.push(q);
                }
            }
        }
        if mixin_quals.is_empty() {
            return None;
        }
        let family = self.receiver_redis_family(&attr.value)?;
        for q in &mixin_quals {
            if let Some(mapped) = map_redis_mixin_to_canonical_sink(q, family) {
                return Some(mapped);
            }
        }
        None
    }

    /// Sync vs asyncio redis for a receiver expression, via the type MRO
    /// (or the enclosing class when the receiver is `super()`).
    /// Memoized per receiver type qualname within a file visit.
    fn receiver_redis_family(&mut self, expr: &Expr) -> Option<RedisClientFamily> {
        if self.expr_is_super(expr) {
            let key = format!(
                "super:{}",
                self.class_stack.last().map(String::as_str).unwrap_or("")
            );
            if let Some(cached) = self.redis_family_memo.get(&key) {
                return *cached;
            }
            let family = self.enclosing_class_redis_family();
            self.redis_family_memo.insert(key, family);
            return family;
        }
        if let Some(q) = self.type_instance_qualname(expr) {
            if let Some(cached) = self.redis_family_memo.get(&q) {
                return *cached;
            }
            let ty = expr.inferred_type(self.model)?;
            let family = self.type_redis_family(ty, &mut HashSet::new());
            self.redis_family_memo.insert(q, family);
            return family;
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
