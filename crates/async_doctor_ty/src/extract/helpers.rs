//! Small call-shape and type-name helpers shared by extraction passes.

use ruff_db::files::FilePath;
use ruff_db::parsed::parsed_module;
use ruff_python_ast::{self as ast, Expr};
use ty_python_core::definition::DefinitionKind;
use ty_python_core::semantic_index;
use ty_python_semantic::SemanticModel;

use crate::cache::third_party_module_path;
use crate::extract::path::module_name_from_relative;

pub(crate) fn is_sync_redis_type(ty: &str) -> bool {
    (ty.contains("redis.") || ty.contains(".redis"))
        && !is_asyncio_redis_type(ty)
        && (ty.contains("Redis") || ty.contains("Pipeline") || ty.contains("StrictRedis"))
}

pub(crate) fn is_redis_commands_mixin(q: &str) -> bool {
    q.contains("redis.commands.core.CoreCommands")
        || q.contains("redis.commands.core.AsyncCoreCommands")
        || q.contains("redis.commands.cluster.")
}

/// True when `q` is a redis commands-core method, including the class-stripped
/// form ty sometimes yields (`redis.commands.core.get`).
pub(crate) fn is_redis_commands_core_method(q: &str) -> bool {
    if q.contains("redis.commands.cluster.") {
        return false;
    }
    is_redis_commands_mixin(q)
        || q.strip_prefix("redis.commands.core.")
            .is_some_and(|rest| !rest.is_empty() && !rest.contains('.'))
}

/// Sync vs asyncio redis client family (from the *receiver* MRO, not the
/// commands-mixin defining class — mixins are shared plain `def`s).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RedisClientFamily {
    Sync,
    Async,
}

/// Map a redis commands-mixin defining-class qualname to the canonical
/// receiver sink key so project subclasses inherit sync/async status.
///
/// Family must come from the receiver type's MRO (`redis.asyncio.Redis` ⇒
/// async), never from whether the mixin method is `async def`.
pub(crate) fn map_redis_mixin_to_canonical_sink(
    q: &str,
    family: RedisClientFamily,
) -> Option<String> {
    if !is_redis_commands_core_method(q) {
        return None;
    }
    let method = q.rsplit('.').next().filter(|m| !m.is_empty())?;
    match family {
        RedisClientFamily::Async => Some(format!("redis.asyncio.Redis.{method}")),
        RedisClientFamily::Sync => Some(format!("redis.Redis.{method}")),
    }
}

pub(crate) fn qualname_from_definition<'db>(
    model: &SemanticModel<'db>,
    def: ty_python_core::definition::Definition<'db>,
) -> Option<String> {
    let db = model.db();
    let parsed = parsed_module(db, def.python_file(db)).load(db);
    let kind = def.kind(db);
    let DefinitionKind::Function(fdef) = kind else {
        // Could be an import binding pointing at a function — use def.name.
        let name = def.name(db)?;
        let module = module_of_definition(model, def)?;
        return Some(format!("{module}.{name}"));
    };
    let node = fdef.node(&parsed);
    let func_name = node.name.as_str();
    let module = module_of_definition(model, def)?;

    // Enclosing class via parent scopes.
    let index = semantic_index(db, def.program_file(db));
    let mut scope_id = def.file_scope(db);
    let mut class_parts: Vec<String> = Vec::new();
    while let Some(parent) = index.parent_scope_id(scope_id) {
        let scope = index.scope(parent);
        if let Some(class_ref) = scope.node().as_class() {
            let class_node = class_ref.node(&parsed);
            class_parts.push(class_node.name.as_str().to_string());
        }
        scope_id = parent;
    }
    class_parts.reverse();
    if class_parts.is_empty() {
        Some(format!("{module}.{func_name}"))
    } else {
        Some(format!("{module}.{}.{}", class_parts.join("."), func_name))
    }
}

pub(crate) fn module_of_definition<'db>(
    model: &SemanticModel<'db>,
    def: ty_python_core::definition::Definition<'db>,
) -> Option<String> {
    let db = model.db();
    let file = def.file(db);
    match file.path(db) {
        FilePath::System(sys) => {
            let s = sys.as_str().replace('\\', "/");
            if let Some(rest) = third_party_module_path(&s) {
                return Some(module_name_from_relative(rest));
            }
            // Best-effort: file stem when we cannot recover the package path.
            Some(module_name_from_relative(
                sys.file_name().unwrap_or(sys.as_str()),
            ))
        }
        FilePath::Vendored(vendored) => {
            let s = vendored.as_str().replace('\\', "/");
            // typeshed: stdlib/time.pyi, stdlib/pathlib/__init__.pyi
            let rest = s
                .strip_prefix("stdlib/")
                .or_else(|| {
                    // stubs/requests/requests/... → skip first component
                    s.strip_prefix("stubs/")
                        .and_then(|r| r.split_once('/').map(|(_, rest)| rest))
                })
                .unwrap_or(&s);
            let rest = rest
                .strip_suffix(".pyi")
                .or_else(|| rest.strip_suffix(".py"))
                .unwrap_or(rest);
            Some(module_name_from_relative(rest))
        }
        FilePath::SystemVirtual(_) => None,
    }
}

pub(crate) fn is_callable_ref(expr: &Expr) -> bool {
    match expr {
        Expr::Name(_) | Expr::Attribute(_) | Expr::Lambda(_) => true,
        Expr::Call(call) => is_partial_call(call),
        _ => false,
    }
}

pub(crate) fn is_partial_call(call: &ast::ExprCall) -> bool {
    match call.func.as_ref() {
        Expr::Name(n) if n.id.as_str() == "partial" => true,
        Expr::Attribute(a) if a.attr.as_str() == "partial" => true,
        _ => false,
    }
}

/// Extract the route handler expression from imperative FastAPI/Starlette registration.
pub(crate) fn route_handler_arg(call: &ast::ExprCall) -> Option<&Expr> {
    let attr_name = match call.func.as_ref() {
        Expr::Attribute(a) => Some(a.attr.as_str()),
        Expr::Name(n) => Some(n.id.as_str()),
        _ => None,
    };
    let name = attr_name?;

    match name {
        "add_api_route" | "add_route" => {
            // add_api_route(path, endpoint, ...) — handler at index 1, or endpoint=
            if let Some(arg) = call.arguments.args.get(1) {
                return Some(arg);
            }
            call.arguments
                .keywords
                .iter()
                .find(|k| k.arg.as_ref().is_some_and(|a| a.as_str() == "endpoint"))
                .map(|k| &k.value)
        }
        "Route" | "WebSocketRoute" => {
            // Route(path, endpoint) or Route(path, endpoint=...)
            if let Some(arg) = call.arguments.args.get(1) {
                return Some(arg);
            }
            if let Some(arg) = call.arguments.args.first() {
                // Rare: Route(endpoint) — only if single positional and not a string.
                if !matches!(arg, Expr::StringLiteral(_)) {
                    return Some(arg);
                }
            }
            call.arguments
                .keywords
                .iter()
                .find(|k| {
                    k.arg
                        .as_ref()
                        .is_some_and(|a| matches!(a.as_str(), "endpoint" | "app"))
                })
                .map(|k| &k.value)
        }
        _ => None,
    }
}

pub(crate) fn is_getattr_call(func: &Expr) -> bool {
    match func {
        Expr::Name(n) => n.id.as_str() == "getattr",
        Expr::Attribute(a) => a.attr.as_str() == "getattr",
        _ => false,
    }
}

/// True for redis.asyncio types (client, pipeline, connection, …).
pub(crate) fn is_asyncio_redis_type(ty: &str) -> bool {
    ty.contains("redis.asyncio") || ty.contains("asyncio.client.Pipeline")
}

/// Canonicalize asyncio-redis type strings so site- and dist-packages agree.
pub(crate) fn normalize_asyncio_redis_type(ty: &str) -> String {
    if ty.contains("Pipeline") {
        return "redis.asyncio.client.Pipeline".to_string();
    }
    if ty.contains("Redis") || ty.ends_with(".StrictRedis") {
        return "redis.asyncio.client.Redis".to_string();
    }
    if let Some(rest) = ty.strip_prefix("redis.asyncio.") {
        return format!("redis.asyncio.{rest}");
    }
    ty.to_string()
}

pub(crate) fn is_task_spawn_callee(func: &Expr) -> bool {
    match func {
        Expr::Attribute(attr) => matches!(
            attr.attr.as_str(),
            "create_task" | "ensure_future" | "gather" | "start_soon" | "start" | "add_task"
        ),
        Expr::Name(name) => matches!(name.id.as_str(), "create_task" | "ensure_future" | "gather"),
        _ => false,
    }
}

/// In-memory buffer / byte-stream types that are not blocking file IO.
pub(crate) fn is_in_memory_buffer_type(ty: &str) -> bool {
    let t = ty;
    t.contains("BytesIO")
        || t.contains("StringIO")
        || t.contains("BufferedIOBase")
        || t.ends_with("bytearray")
        || t.ends_with(".memoryview")
        || t.contains("io.BytesIO")
        || t.contains("io.StringIO")
        || t.contains("_io.BytesIO")
        || t.contains("_io.StringIO")
}

/// Qualnames whose first argument is a path/file and may be an in-memory buffer instead.
pub(crate) fn is_path_or_file_open_qual(q: &str) -> bool {
    matches!(
        q,
        "builtins.open"
            | "io.open"
            | "pathlib.Path.open"
            | "pathlib.Path.read_text"
            | "pathlib.Path.read_bytes"
            | "pathlib.Path.write_text"
            | "pathlib.Path.write_bytes"
    ) || q.ends_with(".Image.open")
        || q.ends_with(".Image.save")
        || q.ends_with("PIL.Image.open")
        || q.ends_with("PIL.Image.save")
}

/// Cheap narrowing facts from `isinstance(x, T)` / `x is not None`.
#[derive(Debug, Clone)]
pub(crate) enum NarrowFact {
    /// `isinstance(name, T)` → use this instance qualname for `name`.
    IsInstance(String),
    /// `name is not None` → drop None from unions when resolving `name`.
    NotNone,
}

/// Extract a single cheap narrowing from an `if` test (no `and`/`or` explosion).
pub(crate) fn narrow_from_test(test: &Expr) -> Option<(String, NarrowFact)> {
    match test {
        Expr::Compare(cmp) if cmp.ops.len() == 1 && cmp.operands.len() == 2 => {
            let op = cmp.ops[0];
            let left = &cmp.operands[0];
            let right = &cmp.operands[1];
            // `x is not None` / `x != None`
            if matches!(op, ast::CmpOp::IsNot | ast::CmpOp::NotEq)
                && matches!(right, Expr::NoneLiteral(_))
                && let Expr::Name(n) = left
            {
                return Some((n.id.to_string(), NarrowFact::NotNone));
            }
            None
        }
        Expr::Call(call) => {
            let is_isinstance = match call.func.as_ref() {
                Expr::Name(n) => n.id.as_str() == "isinstance",
                Expr::Attribute(a) => a.attr.as_str() == "isinstance",
                _ => false,
            };
            if !is_isinstance || call.arguments.args.len() < 2 {
                return None;
            }
            let Expr::Name(n) = &call.arguments.args[0] else {
                return None;
            };
            let type_expr = &call.arguments.args[1];
            let ty_name = match type_expr {
                Expr::Name(tn) => tn.id.to_string(),
                Expr::Attribute(attr) => format_attr_path(attr),
                _ => return None,
            };
            Some((n.id.to_string(), NarrowFact::IsInstance(ty_name)))
        }
        _ => None,
    }
}

fn format_attr_path(attr: &ast::ExprAttribute) -> String {
    let mut parts = vec![attr.attr.as_str().to_string()];
    let mut cur = attr.value.as_ref();
    loop {
        match cur {
            Expr::Attribute(a) => {
                parts.push(a.attr.as_str().to_string());
                cur = a.value.as_ref();
            }
            Expr::Name(n) => {
                parts.push(n.id.to_string());
                break;
            }
            _ => break,
        }
    }
    parts.reverse();
    parts.join(".")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redis_mixin_maps_to_canonical_sinks() {
        assert_eq!(
            map_redis_mixin_to_canonical_sink(
                "redis.commands.core.CoreCommands.get",
                RedisClientFamily::Sync
            )
            .as_deref(),
            Some("redis.Redis.get")
        );
        assert_eq!(
            map_redis_mixin_to_canonical_sink("redis.commands.core.get", RedisClientFamily::Async)
                .as_deref(),
            Some("redis.asyncio.Redis.get")
        );
        assert_eq!(
            map_redis_mixin_to_canonical_sink("redis.commands.core.get", RedisClientFamily::Sync)
                .as_deref(),
            Some("redis.Redis.get")
        );
        assert!(
            map_redis_mixin_to_canonical_sink(
                "redis.commands.cluster.RedisClusterCommands.get",
                RedisClientFamily::Sync
            )
            .is_none()
        );
        assert!(
            map_redis_mixin_to_canonical_sink("redis.Redis.get", RedisClientFamily::Sync).is_none()
        );
    }
}
