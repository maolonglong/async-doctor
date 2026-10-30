//! Family / heuristic matchers for third-party sync APIs.

use async_doctor_core::SinkHit;

use crate::{BuiltinSink, RULE_BLOCKING_DB, RULE_BLOCKING_HTTP};

use super::hit;

pub(super) fn match_family(q: &str) -> Option<SinkHit> {
    // requests Session / module-level HTTP verbs
    if q.starts_with("requests.Session.")
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
    // httpx sync Client
    if q.starts_with("httpx.Client.") {
        return Some(hit(&BuiltinSink::finding(
            "httpx.Client.request",
            RULE_BLOCKING_HTTP,
            "Use `httpx.AsyncClient` instead of sync `httpx.Client`.",
        )));
    }
    // urllib3
    if (q.starts_with("urllib3.PoolManager.")
        || q.starts_with("urllib3.HTTPConnectionPool.")
        || q.starts_with("urllib3.HTTPSConnectionPool.")
        || q == "urllib3.request"
        || q.starts_with("urllib3.util.retry.")
        || q.ends_with("PoolManager.request")
        || q.ends_with(".urlopen"))
        && (q.contains("request") || q.ends_with(".urlopen") || q == "urllib3.request")
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
    // Factories that return clients/pipelines/pubsub handles are not IO.
    if q.ends_with(".pipeline") || q.ends_with(".pubsub") {
        return None;
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
    // redis sync client (not asyncio, not a Pipeline receiver)
    if (q.starts_with("redis.Redis.") || q.starts_with("redis.StrictRedis."))
        && !q.starts_with("redis.asyncio.")
        && !is_redis_sync_pipeline_receiver(q)
    {
        return Some(hit(&BuiltinSink::finding(
            "redis.Redis.execute_command",
            RULE_BLOCKING_DB,
            "Use `redis.asyncio.Redis` instead of sync `redis.Redis`.",
        )));
    }
    // sqlalchemy sync Session / Engine (Engine.begin handled specially via with)
    if q.starts_with("sqlalchemy.orm.Session.")
        || q == "sqlalchemy.orm.Session.execute"
        || q == "sqlalchemy.orm.Session.query"
        || q == "sqlalchemy.orm.Session.commit"
        || q == "sqlalchemy.orm.Session.flush"
        || q == "sqlalchemy.orm.Session.scalar"
        || q == "sqlalchemy.orm.Session.scalars"
    {
        return Some(hit(&BuiltinSink::finding(
            "sqlalchemy.orm.Session.execute",
            RULE_BLOCKING_DB,
            "Use `sqlalchemy.ext.asyncio.AsyncSession` instead of sync `Session`.",
        )));
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
    // pymysql
    if q.starts_with("pymysql.") {
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
    // boto3 / botocore
    if (q.starts_with("boto3.") || q.starts_with("botocore."))
        && (q.contains("client")
            || q.contains("resource")
            || q.contains(".call")
            || q.ends_with("._make_api_call")
            || q.contains("make_api_call")
            || method_looks_like_aws_op(q))
    {
        return Some(hit(&BuiltinSink::finding(
            "botocore.client.BaseClient._make_api_call",
            RULE_BLOCKING_HTTP,
            "Use `aioboto3` / `aiobotocore` instead of sync `boto3`.",
        )));
    }
    // openai sync client
    if !q.contains("Async")
        && (q.starts_with("openai.OpenAI.")
            || q.starts_with("openai.resources.")
            || (q.starts_with("openai.") && q.contains("completions")))
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
    // kafka-python
    if q.starts_with("kafka.KafkaProducer.")
        || q.starts_with("kafka.KafkaConsumer.")
        || q.starts_with("kafka.producer.")
        || q.starts_with("kafka.consumer.")
    {
        return Some(hit(&BuiltinSink::finding(
            "kafka.KafkaProducer.send",
            RULE_BLOCKING_HTTP,
            "Use `aiokafka` instead of sync `kafka-python`.",
        )));
    }
    // elasticsearch sync
    if q.starts_with("elasticsearch.Elasticsearch.")
        || (q.starts_with("elasticsearch.")
            && !q.contains("Async")
            && (q.contains(".search")
                || q.contains(".index")
                || q.contains(".get")
                || q.contains(".update")
                || q.contains(".delete")
                || q.contains(".bulk")
                || q.contains(".perform_request")))
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

pub(super) fn method_looks_like_aws_op(q: &str) -> bool {
    // boto3 dynamic client methods: client.get_object, etc.
    let Some((_, method)) = q.rsplit_once('.') else {
        return false;
    };
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
