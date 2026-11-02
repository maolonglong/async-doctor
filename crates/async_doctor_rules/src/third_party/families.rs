//! Family / heuristic matchers for third-party sync APIs.

use async_doctor_core::SinkHit;

use crate::{BuiltinSink, RULE_BLOCKING_DB, RULE_BLOCKING_HTTP};

use super::hit;

pub(super) fn match_family(q: &str) -> Option<SinkHit> {
    // requests Session / module-level HTTP verbs. `mount` / `get_adapter` are
    // adapter-registry lookups — no IO.
    if (q.starts_with("requests.Session.")
        && !matches!(q, "requests.Session.mount" | "requests.Session.get_adapter"))
        || matches!(
            q,
            "requests.get"
                | "requests.post"
                | "requests.put"
                | "requests.patch"
                | "requests.delete"
                | "requests.head"
                | "requests.options"
                | "requests.request"
        )
    {
        return Some(hit(&BuiltinSink::finding(
            "requests.request",
            RULE_BLOCKING_HTTP,
            "Use `httpx.AsyncClient` (or `aiohttp`) instead of sync `requests`.",
        )));
    }
    // httpx sync Client. `build_request` only constructs the Request — no IO.
    if q.starts_with("httpx.Client.")
        && !matches!(q, "httpx.Client.build_request" | "httpx.Client.mount")
    {
        return Some(hit(&BuiltinSink::finding(
            "httpx.Client.request",
            RULE_BLOCKING_HTTP,
            "Use `httpx.AsyncClient` instead of sync `httpx.Client`.",
        )));
    }
    // urllib3 only — never map stdlib `urllib.request.urlopen` here.
    if q.starts_with("urllib3.")
        && (q == "urllib3.request"
            || q.ends_with("PoolManager.request")
            || q.ends_with(".urlopen")
            || q.starts_with("urllib3.util.retry."))
    {
        return Some(hit(&BuiltinSink::finding(
            "urllib3.request",
            RULE_BLOCKING_HTTP,
            "Use an async HTTP client (`httpx.AsyncClient`), or offload.",
        )));
    }
    // redis: decide by *receiver type* (segment `Pipeline`), not defining-class
    // mixins (`CoreCommands.set` → normalize → `redis.Redis.set`).
    // Sync Pipeline buffering is not IO; only sync Pipeline.execute blocks.
    // `redis.asyncio.*` is handled by `is_async_counterpart` (and awaited calls
    // are rejected in propagate regardless).
    if is_redis_sync_pipeline_receiver(q) {
        if is_redis_pipeline_execute_method(q) {
            return Some(hit(&BuiltinSink::finding(
                "redis.client.Pipeline.execute",
                RULE_BLOCKING_DB,
                "Use `redis.asyncio` pipelines (`await pipe.execute()`), or offload.",
            )));
        }
        // pipe.set / pipe.get / client.pipeline() — buffer only.
        return None;
    }
    // Factories that return clients/pipelines/pubsub/lock handles are not IO.
    if q.ends_with(".pipeline") || q.ends_with(".pubsub") || is_redis_client_factory(q) {
        return None;
    }
    // redis.lock.Lock.acquire blocks (network); never share the threading
    // non-blocking exemption — this is a real sync-IO sink.
    if is_redis_sync_lock_acquire(q) {
        return Some(hit(&BuiltinSink::finding(
            "redis.Lock.acquire",
            RULE_BLOCKING_DB,
            "Use `redis.asyncio` locks, or offload lock acquisition.",
        )));
    }
    // redis sync PubSub.listen() — blocks only when the generator is consumed.
    if (q == "redis.PubSub.listen"
        || q == "redis.client.PubSub.listen"
        || q.ends_with(".PubSub.listen"))
        && !q.contains("asyncio")
    {
        return Some(hit(&BuiltinSink::finding_consume_only(
            "redis.PubSub.listen",
            RULE_BLOCKING_DB,
            "Use `redis.asyncio` pub/sub (`async for msg in pubsub.listen()`), or offload.",
        )));
    }
    // Other sync PubSub command methods (subscribe/get_message/…) block on use.
    if (q.starts_with("redis.PubSub.") || q.starts_with("redis.client.PubSub."))
        && !q.contains("asyncio")
        && !q.ends_with(".listen")
    {
        return Some(hit(&BuiltinSink::finding(
            "redis.PubSub.get_message",
            RULE_BLOCKING_DB,
            "Use `redis.asyncio` pub/sub instead of sync `PubSub`.",
        )));
    }
    // redis sync client (not asyncio, not a Pipeline receiver, not a factory)
    if (q.starts_with("redis.Redis.") || q.starts_with("redis.StrictRedis."))
        && !q.starts_with("redis.asyncio.")
        && !is_redis_sync_pipeline_receiver(q)
        && !is_redis_client_factory(q)
    {
        let method = q
            .rsplit_once('.')
            .map(|(_, m)| m)
            .unwrap_or("execute_command");
        let sink_qualname = redis_command_sink_qualname(method);
        return Some(SinkHit {
            rule: RULE_BLOCKING_DB,
            sink_qualname,
            fix_hint: "Use `redis.asyncio.Redis` instead of sync `redis.Redis`.",
            candidate_only: false,
            candidate_reason: None,
            with_only: false,
            consume_only: false,
        });
    }
    // sqlalchemy sync Session: only methods that emit SQL / talk to the DB.
    // Identity-map helpers (`add`, `expunge`, `_is_clean`, …) are not sinks.
    if let Some(method) = sqlalchemy_session_io_method(q) {
        let sink_qualname = sqlalchemy_session_sink_qualname(method);
        return Some(SinkHit {
            rule: RULE_BLOCKING_DB,
            sink_qualname,
            fix_hint: "Use `sqlalchemy.ext.asyncio.AsyncSession` instead of sync `Session`.",
            candidate_only: false,
            candidate_reason: None,
            with_only: false,
            consume_only: false,
        });
    }
    // Legacy Query: iteration / fetch emits SQL (`for u in session.query(User)`).
    if let Some(method) = sqlalchemy_query_io(q) {
        let sink_qualname = if method == "__iter__" {
            "sqlalchemy.orm.Query.__iter__"
        } else {
            intern_qualname(&format!("sqlalchemy.orm.Query.{method}"))
        };
        return Some(SinkHit {
            rule: RULE_BLOCKING_DB,
            sink_qualname,
            fix_hint: "Use `AsyncSession` / `await session.scalars(...)`, or offload.",
            candidate_only: false,
            candidate_reason: None,
            with_only: false,
            consume_only: false,
        });
    }
    if q.starts_with("sqlalchemy.engine.Engine.")
        && !q.contains("begin")
        && (q.ends_with(".connect") || q.ends_with(".execute") || q.ends_with(".raw_connection"))
    {
        return Some(hit(&BuiltinSink::finding(
            "sqlalchemy.engine.Engine.connect",
            RULE_BLOCKING_DB,
            "Use `sqlalchemy.ext.asyncio.create_async_engine` / `AsyncEngine`.",
        )));
    }
    // Engine.begin / Connection — only meaningful when used; Connection.execute always blocks
    if q == "sqlalchemy.engine.Connection.execute"
        || q == "sqlalchemy.engine.Connection.exec_driver_sql"
        || q == "sqlalchemy.engine.Connection.scalar"
    {
        return Some(hit(&BuiltinSink::finding(
            "sqlalchemy.engine.Connection.execute",
            RULE_BLOCKING_DB,
            "Use async SQLAlchemy (`AsyncConnection`) or offload.",
        )));
    }
    // Engine.begin as context manager enter (with_only).
    if q == "sqlalchemy.engine.Engine.begin"
        || q == "sqlalchemy.engine.Engine.begin.__enter__"
        || q.ends_with("Engine.begin")
    {
        return Some(hit(&BuiltinSink::finding_with_only(
            "sqlalchemy.engine.Engine.begin",
            RULE_BLOCKING_DB,
            "Use `AsyncEngine.begin()` / async SQLAlchemy, or offload. \
             (`Engine.begin` blocks when used directly in `with`.)",
        )));
    }
    // psycopg / psycopg2
    if (q.starts_with("psycopg2.")
        || (q.starts_with("psycopg.") && !q.starts_with("psycopg.Async")))
        && (q.contains("connect")
            || q.contains("execute")
            || q.contains("fetch")
            || q.contains("cursor")
            || q.contains("commit"))
    {
        return Some(hit(&BuiltinSink::finding(
            "psycopg.connect",
            RULE_BLOCKING_DB,
            "Use `psycopg` async (`AsyncConnection`) / `asyncpg`, or offload.",
        )));
    }
    // pymysql — exception constructors, constants and pure converters are
    // not IO; only the driver surface below stays a sink.
    if q.starts_with("pymysql.")
        && !q.starts_with("pymysql.err.")
        && !q.starts_with("pymysql.constants.")
        && !q.starts_with("pymysql.converters.")
    {
        return Some(hit(&BuiltinSink::finding(
            "pymysql.connect",
            RULE_BLOCKING_DB,
            "Use `aiomysql` / `asyncmy`, or offload.",
        )));
    }
    // pymongo sync
    if q.starts_with("pymongo.")
        && !q.starts_with("pymongo.asynchronous.")
        && (q.contains("find")
            || q.contains("insert")
            || q.contains("update")
            || q.contains("delete")
            || q.contains("aggregate")
            || q.ends_with(".MongoClient")
            || q.contains("MongoClient"))
    {
        return Some(hit(&BuiltinSink::finding(
            "pymongo.collection.Collection.find",
            RULE_BLOCKING_DB,
            "Use `motor` (async) instead of sync `pymongo`.",
        )));
    }
    // boto3 / botocore — factories + dynamic client/resource ops only.
    // Avoid matching arbitrary `*.client.*` helpers (e.g. BaseClient.meta).
    // `boto3-stubs.*` arrives pre-normalized via [`super::normalize_third_party`].
    // Factories use the `boto3.client` / `boto3.resource` sink keys so they are
    // not deduped against later `BaseClient.*` AWS-op findings on the same entry.
    if is_sync_boto_package_qual(q) {
        if is_sync_boto_factory_qualname(q) {
            let key = if q.contains("resource") {
                "boto3.resource"
            } else {
                "boto3.client"
            };
            return Some(hit(&BuiltinSink::finding(
                key,
                RULE_BLOCKING_HTTP,
                "Use `aioboto3` / `aiobotocore` instead of sync `boto3`.",
            )));
        }
        // AWS-op name heuristic only on client/resource receivers — never
        // `botocore.session.*` / ComponentLocator / other internals (`get_*`).
        let is_api = q.ends_with("._make_api_call")
            || q.contains("make_api_call")
            || (is_boto_client_or_resource_api_qual(q) && method_looks_like_aws_op(q));
        if is_api {
            return Some(hit(&BuiltinSink::finding(
                "botocore.client.BaseClient._make_api_call",
                RULE_BLOCKING_HTTP,
                "Use `aioboto3` / `aiobotocore` instead of sync `boto3`.",
            )));
        }
    }
    // openai sync client / sync resource *public* methods only — never
    // `openai.lib._parsing._completions.*`, private `_Stream` helpers, or
    // anything that merely contains the substring "completions".
    // `with_options` / `copy` return new client objects — no request.
    let client_copy = matches!(
        q,
        "openai.OpenAI.with_options"
            | "openai.OpenAI.copy"
            | "openai.AzureOpenAI.with_options"
            | "openai.AzureOpenAI.copy"
            | "openai.lib.azure.AzureOpenAI.with_options"
            | "openai.lib.azure.AzureOpenAI.copy"
            | "anthropic.Anthropic.with_options"
            | "anthropic.Anthropic.copy"
    );
    if client_copy {
        return None;
    }
    if !q.contains("Async")
        && (q.starts_with("openai.AzureOpenAI.") || q.starts_with("openai.lib.azure.AzureOpenAI."))
    {
        return Some(hit(&BuiltinSink::finding(
            "openai.AzureOpenAI",
            RULE_BLOCKING_HTTP,
            "Use `openai.AsyncAzureOpenAI` instead of sync `AzureOpenAI`.",
        )));
    }
    if !q.contains("Async")
        && (q.starts_with("openai.OpenAI.")
            || (q.starts_with("openai.resources.")
                && !q.split('.').any(|seg| seg.starts_with('_'))))
    {
        return Some(hit(&BuiltinSink::finding(
            "openai.OpenAI",
            RULE_BLOCKING_HTTP,
            "Use `openai.AsyncOpenAI` instead of sync `OpenAI`.",
        )));
    }
    // anthropic sync
    if q.starts_with("anthropic.Anthropic.")
        || (q.starts_with("anthropic.resources.") && !q.contains("Async"))
    {
        return Some(hit(&BuiltinSink::finding(
            "anthropic.Anthropic",
            RULE_BLOCKING_HTTP,
            "Use `anthropic.AsyncAnthropic` instead of sync `Anthropic`.",
        )));
    }
    // kafka-python. Consumer local-state mutators and metrics snapshots do no
    // broker RPC: pause/resume/seek/assignment/subscription/paused/metrics and
    // bootstrap_connected on either class spelling.
    let method = q.rsplit_once('.').map(|(_, m)| m).unwrap_or("");
    let kafka_non_io = (q.contains("KafkaConsumer.")
        && matches!(
            method,
            "pause"
                | "resume"
                | "seek"
                | "assignment"
                | "subscription"
                | "paused"
                | "metrics"
                | "bootstrap_connected"
        ))
        || (q.contains("KafkaProducer.") && matches!(method, "metrics" | "bootstrap_connected"));
    if q.starts_with("kafka.KafkaProducer.")
        || q.starts_with("kafka.KafkaConsumer.")
        || q.starts_with("kafka.producer.")
        || q.starts_with("kafka.consumer.")
    {
        if kafka_non_io {
            return None;
        }
        return Some(hit(&BuiltinSink::finding(
            "kafka.KafkaProducer.send",
            RULE_BLOCKING_HTTP,
            "Use `aiokafka` instead of sync `kafka-python`.",
        )));
    }
    // elasticsearch sync (never `_async` package / `Async*` types)
    if q.starts_with("elasticsearch.")
        && !q.contains("Async")
        && !q.contains("._async.")
        && (q.starts_with("elasticsearch.Elasticsearch.")
            || q.contains(".search")
            || q.contains(".index")
            || q.contains(".get")
            || q.contains(".update")
            || q.contains(".delete")
            || q.contains(".bulk")
            || q.contains(".perform_request"))
    {
        return Some(hit(&BuiltinSink::finding(
            "elasticsearch.Elasticsearch.search",
            RULE_BLOCKING_HTTP,
            "Use `elasticsearch.AsyncElasticsearch` instead of sync client.",
        )));
    }
    // grpc sync stubs
    if q.starts_with("grpc.")
        && !q.contains("aio")
        && (q.contains("UnaryUnaryMultiCallable")
            || q.ends_with(".__call__")
            || q.contains(".future")
            || q.contains("Channel")
            || q.contains("insecure_channel")
            || q.contains("secure_channel"))
    {
        return Some(hit(&BuiltinSink::finding(
            "grpc.Channel",
            RULE_BLOCKING_HTTP,
            "Use `grpc.aio` channel/stubs instead of sync gRPC.",
        )));
    }
    None
}

/// True when the External qualname is a method on a *sync* redis Pipeline receiver.
///
/// Expects receiver-based keys like `redis.client.Pipeline.set` (not mixin
/// `redis.commands.core.CoreCommands.set`). Async pipelines live under
/// `redis.asyncio` and are excluded here.
pub(super) fn is_redis_sync_pipeline_receiver(q: &str) -> bool {
    if q.contains("asyncio") {
        return false;
    }
    // Path segment exactly `Pipeline` (redis.client.Pipeline / redis.Pipeline / …).
    q.split('.').any(|seg| seg == "Pipeline")
}

pub(super) fn is_redis_pipeline_execute_method(q: &str) -> bool {
    q.ends_with(".execute")
        || q.ends_with(".execute_command")
        || q.ends_with(".immediate_execute_command")
}

/// `client.pipeline()` / `pipe.set()` / `pipe.get()` only buffer; sync `pipe.execute()` blocks.
pub(super) fn is_redis_pipeline_buffering(q: &str) -> bool {
    if !is_redis_sync_pipeline_receiver(q) {
        return false;
    }
    !is_redis_pipeline_execute_method(q)
}

/// Client constructors / pool helpers — no network until a command runs.
fn is_redis_client_factory(q: &str) -> bool {
    let method = q.rsplit_once('.').map(|(_, m)| m).unwrap_or(q);
    matches!(
        method,
        "from_url" | "from_pool" | "__init__" | "__new__" | "Redis" | "StrictRedis" | "lock" // redis.Redis.lock(...) builds a Lock handle; no IO yet
    )
}

fn is_redis_sync_lock_acquire(q: &str) -> bool {
    // redis.Lock / redis.client.Lock / redis.lock.Lock — not threading.Lock.
    if q.contains("asyncio") || !q.starts_with("redis.") {
        return false;
    }
    q.split('.').any(|seg| seg == "Lock") && (q.ends_with(".acquire") || q.ends_with(".__enter__"))
}

/// Label redis sync commands with the concrete method (not a blanket `execute_command`).
fn redis_command_sink_qualname(method: &str) -> &'static str {
    match method {
        "get" => "redis.Redis.get",
        "set" => "redis.Redis.set",
        "delete" => "redis.Redis.delete",
        "ping" => "redis.Redis.ping",
        "execute_command" => "redis.Redis.execute_command",
        other => intern_qualname(&format!("redis.Redis.{other}")),
    }
}

fn intern_qualname(s: &str) -> &'static str {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    static CACHE: OnceLock<Mutex<HashMap<String, &'static str>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut guard = cache.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(&existing) = guard.get(s) {
        return existing;
    }
    let leaked: &'static str = Box::leak(s.to_owned().into_boxed_str());
    guard.insert(s.to_owned(), leaked);
    leaked
}

/// Session methods that talk to the DB / emit SQL (SQLAlchemy 2.0 Session API).
///
/// Non-IO identity-map helpers (`add`, `expunge`, `expire`, `_is_clean`, …) return
/// `None` so they are neither sinks nor follow shortcuts.
fn sqlalchemy_session_io_method(q: &str) -> Option<&'static str> {
    let method = q
        .strip_prefix("sqlalchemy.orm.Session.")
        .or_else(|| q.strip_prefix("sqlalchemy.orm.session.Session."))?;
    // Ignore dunder / private helpers reached via follow into Session internals.
    if method.starts_with('_') {
        return None;
    }
    match method {
        "execute" => Some("execute"),
        "scalar" => Some("scalar"),
        "scalars" => Some("scalars"),
        "get" => Some("get"),
        "get_one" => Some("get_one"),
        "get_one_or_none" => Some("get_one_or_none"),
        "commit" => Some("commit"),
        "flush" => Some("flush"),
        "rollback" => Some("rollback"),
        "refresh" => Some("refresh"),
        "connection" => Some("connection"),
        "merge" => Some("merge"),
        "close" => Some("close"),
        "scalar_one" => Some("scalar_one"),
        "scalar_one_or_none" => Some("scalar_one_or_none"),
        "bulk_save_objects" => Some("bulk_save_objects"),
        "bulk_insert_mappings" => Some("bulk_insert_mappings"),
        "bulk_update_mappings" => Some("bulk_update_mappings"),
        _ => None,
    }
}

fn sqlalchemy_session_sink_qualname(method: &str) -> &'static str {
    match method {
        "execute" => "sqlalchemy.orm.Session.execute",
        "scalar" => "sqlalchemy.orm.Session.scalar",
        "scalars" => "sqlalchemy.orm.Session.scalars",
        "get" => "sqlalchemy.orm.Session.get",
        "get_one" => "sqlalchemy.orm.Session.get_one",
        "get_one_or_none" => "sqlalchemy.orm.Session.get_one_or_none",
        "commit" => "sqlalchemy.orm.Session.commit",
        "flush" => "sqlalchemy.orm.Session.flush",
        "rollback" => "sqlalchemy.orm.Session.rollback",
        "refresh" => "sqlalchemy.orm.Session.refresh",
        "connection" => "sqlalchemy.orm.Session.connection",
        "merge" => "sqlalchemy.orm.Session.merge",
        "close" => "sqlalchemy.orm.Session.close",
        "scalar_one" => "sqlalchemy.orm.Session.scalar_one",
        "scalar_one_or_none" => "sqlalchemy.orm.Session.scalar_one_or_none",
        "bulk_save_objects" => "sqlalchemy.orm.Session.bulk_save_objects",
        "bulk_insert_mappings" => "sqlalchemy.orm.Session.bulk_insert_mappings",
        "bulk_update_mappings" => "sqlalchemy.orm.Session.bulk_update_mappings",
        _ => "sqlalchemy.orm.Session.execute",
    }
}

/// Legacy `Query` iteration / fetch APIs emit SQL when consumed.
fn sqlalchemy_query_io(q: &str) -> Option<&'static str> {
    let method = q
        .strip_prefix("sqlalchemy.orm.Query.")
        .or_else(|| q.strip_prefix("sqlalchemy.orm.query.Query."))?;
    match method {
        "__iter__" => Some("__iter__"),
        "all" => Some("all"),
        "first" => Some("first"),
        "one" => Some("one"),
        "one_or_none" => Some("one_or_none"),
        "scalar" => Some("scalar"),
        "get" => Some("get"),
        "count" => Some("count"),
        "delete" => Some("delete"),
        "update" => Some("update"),
        _ => None,
    }
}

/// Strip a trailing `-stubs` from the top-level package segment without
/// allocating when no `-stubs` suffix is present.
pub(super) fn normalize_stubs_package_root_cow(qualname: &str) -> std::borrow::Cow<'_, str> {
    let Some((root, rest)) = qualname.split_once('.') else {
        return std::borrow::Cow::Borrowed(qualname.strip_suffix("-stubs").unwrap_or(qualname));
    };
    match root.strip_suffix("-stubs") {
        Some(stripped) => std::borrow::Cow::Owned(format!("{stripped}.{rest}")),
        None => std::borrow::Cow::Borrowed(qualname),
    }
}

/// Strip a trailing `-stubs` from the top-level package segment.
///
/// `boto3-stubs.session.Session.client` → `boto3.session.Session.client`,
/// `aioboto3-stubs.session.Session` → `aioboto3.session.Session`.
pub fn normalize_stubs_package_root(qualname: &str) -> String {
    normalize_stubs_package_root_cow(qualname).into_owned()
}

/// True for sync `boto3` / `botocore` package roots (after `-stubs` normalize).
pub fn is_sync_boto_package_qual(qualname: &str) -> bool {
    let q = normalize_stubs_package_root_cow(qualname);
    if is_aioboto_package_qual(&q) {
        return false;
    }
    q == "boto3" || q == "botocore" || q.starts_with("boto3.") || q.starts_with("botocore.")
}

/// True for `aioboto3` / `aiobotocore` roots (including `*-stubs` packages).
pub fn is_aioboto_package_qual(qualname: &str) -> bool {
    let q = normalize_stubs_package_root_cow(qualname);
    q == "aioboto3"
        || q == "aiobotocore"
        || q.starts_with("aioboto3.")
        || q.starts_with("aiobotocore.")
}

/// Sync `boto3.client` / `Session.client` factories — never aioboto3.
pub fn is_sync_boto_factory_qualname(qualname: &str) -> bool {
    let q = normalize_stubs_package_root_cow(qualname);
    if is_aioboto_package_qual(&q) {
        return false;
    }
    matches!(
        q.as_ref(),
        "boto3.client"
            | "boto3.resource"
            | "boto3.session.Session.client"
            | "boto3.session.Session.resource"
            | "boto3.Session.client"
            | "boto3.Session.resource"
    ) || ((q.starts_with("boto3.") || q.starts_with("botocore."))
        && (q.ends_with(".Session.client") || q.ends_with(".Session.resource")))
}

/// Sync AWS client/resource API method names (boto3 has no per-op stubs).
/// Excludes meta helpers (`get_paginator`, `get_waiter`, …) that are not IO.
pub fn aws_op_method_name(method: &str) -> bool {
    if matches!(
        method,
        "get_paginator"
            | "get_waiter"
            | "can_paginate"
            | "generate_presigned_url"
            | "get_available_regions"
            | "meta"
            | "exceptions"
            | "waiter_names"
            | "get_available_subresources"
    ) {
        return false;
    }
    matches!(
        method,
        "get_object"
            | "put_object"
            | "list_objects"
            | "list_objects_v2"
            | "delete_object"
            | "send_message"
            | "receive_message"
            | "publish"
            | "get_item"
            | "put_item"
            | "query"
            | "scan"
            | "invoke"
            | "start_execution"
            | "describe_instances"
            | "download_file"
            | "upload_file"
            | "download_fileobj"
            | "upload_fileobj"
    ) || method.starts_with("get_")
        || method.starts_with("put_")
        || method.starts_with("list_")
        || method.starts_with("create_")
        || method.starts_with("delete_")
        || method.starts_with("describe_")
        || method.starts_with("update_")
}

/// True when `qualname` is a sync boto client or resource API surface.
///
/// Operation-name heuristics must not apply to `botocore.session.*`,
/// `ComponentLocator`, or other non-client internals — those expose many
/// `get_*` helpers that are local metadata, not HTTP.
pub fn is_boto_client_or_resource_api_qual(qualname: &str) -> bool {
    let q = normalize_stubs_package_root_cow(qualname);
    if is_aioboto_package_qual(&q) {
        return false;
    }
    // `botocore.client.BaseClient.*` and dynamically created client classes.
    if q.starts_with("botocore.client.") {
        return true;
    }
    // Raw resource factory types, and post-normalize `boto3.resources.factory.`
    // → `boto3.` forms that still live under the resources package.
    if q.starts_with("boto3.resources.") {
        return true;
    }
    false
}

pub(super) fn method_looks_like_aws_op(q: &str) -> bool {
    // boto3 dynamic client methods: client.get_object, etc.
    q.rsplit_once('.')
        .is_some_and(|(_, method)| aws_op_method_name(method))
}
