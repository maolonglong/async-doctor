//! Built-in third-party blocking sinks (type-based matching).
//!
//! Used both as a shortcut (skip following into known libraries) and for C
//! extensions without source. Async counterparts must never match.

use async_doctor_core::{SinkHit, SinkTable};

use crate::{BuiltinSink, RULE_BLOCKING_DB, RULE_BLOCKING_HTTP, StdlibSinkTable};

/// Combined stdlib + third-party sink table.
#[derive(Debug, Default, Clone)]
pub struct BuiltinSinkTable {
    stdlib: StdlibSinkTable,
}

impl BuiltinSinkTable {
    pub fn new() -> Self {
        Self {
            stdlib: StdlibSinkTable::new(),
        }
    }
}

impl SinkTable for BuiltinSinkTable {
    fn lookup(&self, qualname: &str) -> Option<SinkHit> {
        if is_async_counterpart(qualname) {
            return None;
        }
        if let Some(hit) = lookup_third_party(qualname) {
            return Some(hit);
        }
        self.stdlib.lookup(qualname)
    }
}

fn hit(sink: &BuiltinSink) -> SinkHit {
    sink.hit()
}

/// True for known async APIs that must never fire as blocking sinks.
pub fn is_async_counterpart(qualname: &str) -> bool {
    let q = qualname;
    // httpx
    if q.starts_with("httpx.AsyncClient.") || q == "httpx.AsyncClient" {
        return true;
    }
    // aiohttp
    if q.starts_with("aiohttp.") {
        return true;
    }
    // redis.asyncio
    if q.starts_with("redis.asyncio.") {
        return true;
    }
    // sqlalchemy async
    if q.contains("AsyncSession")
        || q.contains("AsyncEngine")
        || q.contains("AsyncConnection")
        || q.starts_with("sqlalchemy.ext.asyncio.")
    {
        return true;
    }
    // databases / asyncpg / motor / aiomysql / aiobotocore / aioboto3
    if q.starts_with("asyncpg.")
        || q.starts_with("motor.")
        || q.starts_with("aiomysql.")
        || q.starts_with("aiobotocore.")
        || q.starts_with("aioboto3.")
        || q.starts_with("aiofiles.")
    {
        return true;
    }
    // openai / anthropic async clients
    if q.starts_with("openai.AsyncOpenAI")
        || q.starts_with("openai.AsyncAzureOpenAI")
        || q.starts_with("anthropic.AsyncAnthropic")
        || q.starts_with("anthropic.AsyncAnthropicBedrock")
    {
        return true;
    }
    // elasticsearch async
    if q.starts_with("elasticsearch.AsyncElasticsearch") || q.starts_with("elasticsearch.Async") {
        return true;
    }
    // grpc aio
    if q.contains("grpc.aio") || q.starts_with("grpc.aio.") {
        return true;
    }
    // psycopg async (psycopg3 AsyncConnection)
    if q.starts_with("psycopg.AsyncConnection")
        || q.starts_with("psycopg.AsyncCursor")
        || q.starts_with("psycopg_pool.AsyncConnectionPool")
    {
        return true;
    }
    false
}

fn lookup_third_party(qualname: &str) -> Option<SinkHit> {
    // Pipeline buffering methods are never sinks (even if aliased to Redis.*).
    if is_redis_pipeline_buffering(qualname) {
        return None;
    }
    let normalized = normalize_third_party(qualname);
    if is_redis_pipeline_buffering(&normalized) {
        return None;
    }
    for sink in THIRD_PARTY_SINKS {
        if sink.qualname == normalized || sink.qualname == qualname {
            return Some(hit(sink));
        }
    }
    for (alias, canonical) in THIRD_PARTY_ALIASES {
        if (normalized == *alias || qualname == *alias)
            && let Some(sink) = THIRD_PARTY_SINKS.iter().find(|s| s.qualname == *canonical)
        {
            return Some(hit(sink));
        }
    }
    // Prefix / suffix heuristics for method families on known sync types.
    if let Some(hit) = match_family(&normalized) {
        return Some(hit);
    }
    None
}

fn normalize_third_party(qualname: &str) -> String {
    let mut q = qualname.to_string();
    // Common stub / runtime layout aliases.
    let replacements = [
        ("requests.sessions.Session.", "requests.Session."),
        ("requests.api.", "requests."),
        ("urllib3._request_methods.", "urllib3."),
        ("urllib3.connectionpool.", "urllib3."),
        ("httpx._client.", "httpx."),
        ("redis.client.", "redis."),
        ("redis.commands.core.", "redis.Redis."),
        ("sqlalchemy.engine.base.", "sqlalchemy.engine."),
        ("sqlalchemy.orm.session.", "sqlalchemy.orm."),
        ("botocore.client.", "botocore.client."),
        ("boto3.resources.factory.", "boto3."),
        ("openai.resources.", "openai.resources."),
        ("anthropic.resources.", "anthropic.resources."),
        ("kafka.producer.kafka.", "kafka."),
        ("kafka.consumer.group.", "kafka."),
        ("elasticsearch.client.", "elasticsearch."),
        ("pymongo.collection.", "pymongo."),
        ("pymongo.database.", "pymongo."),
        ("pymongo.mongo_client.", "pymongo."),
        ("psycopg2._psycopg.", "psycopg2."),
        ("psycopg.connection.", "psycopg."),
        ("psycopg.cursor.", "psycopg."),
        ("pymysql.connections.", "pymysql."),
        ("pymysql.cursors.", "pymysql."),
        ("grpc._channel.", "grpc."),
    ];
    for (from, to) in replacements {
        if let Some(rest) = q.strip_prefix(from) {
            q = format!("{to}{rest}");
        }
    }
    q
}

fn match_family(q: &str) -> Option<SinkHit> {
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
    if q.ends_with(".pipeline") {
        return None;
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
fn is_redis_sync_pipeline_receiver(q: &str) -> bool {
    if q.contains("asyncio") {
        return false;
    }
    // Path segment exactly `Pipeline` (redis.client.Pipeline / redis.Pipeline / …).
    q.split('.').any(|seg| seg == "Pipeline")
}

fn is_redis_pipeline_execute_method(q: &str) -> bool {
    q.ends_with(".execute")
        || q.ends_with(".execute_command")
        || q.ends_with(".immediate_execute_command")
}

/// `client.pipeline()` / `pipe.set()` / `pipe.get()` only buffer; sync `pipe.execute()` blocks.
fn is_redis_pipeline_buffering(q: &str) -> bool {
    if !is_redis_sync_pipeline_receiver(q) {
        return false;
    }
    !is_redis_pipeline_execute_method(q)
}

fn method_looks_like_aws_op(q: &str) -> bool {
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

/// Explicit third-party sink rows (plus family matchers above).
pub static THIRD_PARTY_SINKS: &[BuiltinSink] = &[
    // --- requests ---
    BuiltinSink::finding(
        "requests.request",
        RULE_BLOCKING_HTTP,
        "Use `httpx.AsyncClient` (or `aiohttp`) instead of sync `requests`.",
    ),
    BuiltinSink::finding(
        "requests.get",
        RULE_BLOCKING_HTTP,
        "Use `httpx.AsyncClient` (or `aiohttp`) instead of sync `requests`.",
    ),
    BuiltinSink::finding(
        "requests.post",
        RULE_BLOCKING_HTTP,
        "Use `httpx.AsyncClient` (or `aiohttp`) instead of sync `requests`.",
    ),
    BuiltinSink::finding(
        "requests.put",
        RULE_BLOCKING_HTTP,
        "Use `httpx.AsyncClient` (or `aiohttp`) instead of sync `requests`.",
    ),
    BuiltinSink::finding(
        "requests.patch",
        RULE_BLOCKING_HTTP,
        "Use `httpx.AsyncClient` (or `aiohttp`) instead of sync `requests`.",
    ),
    BuiltinSink::finding(
        "requests.delete",
        RULE_BLOCKING_HTTP,
        "Use `httpx.AsyncClient` (or `aiohttp`) instead of sync `requests`.",
    ),
    BuiltinSink::finding(
        "requests.head",
        RULE_BLOCKING_HTTP,
        "Use `httpx.AsyncClient` (or `aiohttp`) instead of sync `requests`.",
    ),
    BuiltinSink::finding(
        "requests.Session.request",
        RULE_BLOCKING_HTTP,
        "Use `httpx.AsyncClient` instead of sync `requests.Session`.",
    ),
    BuiltinSink::finding(
        "requests.Session.get",
        RULE_BLOCKING_HTTP,
        "Use `httpx.AsyncClient` instead of sync `requests.Session`.",
    ),
    BuiltinSink::finding(
        "requests.Session.post",
        RULE_BLOCKING_HTTP,
        "Use `httpx.AsyncClient` instead of sync `requests.Session`.",
    ),
    // --- httpx sync ---
    BuiltinSink::finding(
        "httpx.Client.request",
        RULE_BLOCKING_HTTP,
        "Use `httpx.AsyncClient` instead of sync `httpx.Client`.",
    ),
    BuiltinSink::finding(
        "httpx.Client.get",
        RULE_BLOCKING_HTTP,
        "Use `httpx.AsyncClient` instead of sync `httpx.Client`.",
    ),
    BuiltinSink::finding(
        "httpx.Client.post",
        RULE_BLOCKING_HTTP,
        "Use `httpx.AsyncClient` instead of sync `httpx.Client`.",
    ),
    BuiltinSink::finding(
        "httpx.Client.send",
        RULE_BLOCKING_HTTP,
        "Use `httpx.AsyncClient` instead of sync `httpx.Client`.",
    ),
    BuiltinSink::finding(
        "httpx.get",
        RULE_BLOCKING_HTTP,
        "Use `httpx.AsyncClient` instead of sync module-level `httpx.get`.",
    ),
    BuiltinSink::finding(
        "httpx.post",
        RULE_BLOCKING_HTTP,
        "Use `httpx.AsyncClient` instead of sync module-level `httpx.post`.",
    ),
    BuiltinSink::finding(
        "httpx.request",
        RULE_BLOCKING_HTTP,
        "Use `httpx.AsyncClient` instead of sync module-level `httpx.request`.",
    ),
    // --- urllib3 ---
    BuiltinSink::finding(
        "urllib3.request",
        RULE_BLOCKING_HTTP,
        "Use an async HTTP client (`httpx.AsyncClient`), or offload.",
    ),
    BuiltinSink::finding(
        "urllib3.PoolManager.request",
        RULE_BLOCKING_HTTP,
        "Use an async HTTP client (`httpx.AsyncClient`), or offload.",
    ),
    BuiltinSink::finding(
        "urllib3.HTTPConnectionPool.urlopen",
        RULE_BLOCKING_HTTP,
        "Use an async HTTP client (`httpx.AsyncClient`), or offload.",
    ),
    // --- redis sync ---
    BuiltinSink::finding(
        "redis.Redis.execute_command",
        RULE_BLOCKING_DB,
        "Use `redis.asyncio.Redis` instead of sync `redis.Redis`.",
    ),
    BuiltinSink::finding(
        "redis.Redis.get",
        RULE_BLOCKING_DB,
        "Use `redis.asyncio.Redis` instead of sync `redis.Redis`.",
    ),
    BuiltinSink::finding(
        "redis.Redis.set",
        RULE_BLOCKING_DB,
        "Use `redis.asyncio.Redis` instead of sync `redis.Redis`.",
    ),
    BuiltinSink::finding(
        "redis.Redis.delete",
        RULE_BLOCKING_DB,
        "Use `redis.asyncio.Redis` instead of sync `redis.Redis`.",
    ),
    BuiltinSink::finding(
        "redis.Redis.ping",
        RULE_BLOCKING_DB,
        "Use `redis.asyncio.Redis` instead of sync `redis.Redis`.",
    ),
    // Pipeline.execute is the only buffering→IO boundary.
    BuiltinSink::finding(
        "redis.client.Pipeline.execute",
        RULE_BLOCKING_DB,
        "Use `redis.asyncio` pipelines (`await pipe.execute()`), or offload.",
    ),
    // --- sqlalchemy sync ---
    BuiltinSink::finding(
        "sqlalchemy.orm.Session.execute",
        RULE_BLOCKING_DB,
        "Use `sqlalchemy.ext.asyncio.AsyncSession` instead of sync `Session`.",
    ),
    BuiltinSink::finding(
        "sqlalchemy.orm.Session.commit",
        RULE_BLOCKING_DB,
        "Use `sqlalchemy.ext.asyncio.AsyncSession` instead of sync `Session`.",
    ),
    BuiltinSink::finding(
        "sqlalchemy.orm.Session.scalar",
        RULE_BLOCKING_DB,
        "Use `sqlalchemy.ext.asyncio.AsyncSession` instead of sync `Session`.",
    ),
    BuiltinSink::finding(
        "sqlalchemy.engine.Engine.connect",
        RULE_BLOCKING_DB,
        "Use `sqlalchemy.ext.asyncio.create_async_engine` / `AsyncEngine`.",
    ),
    BuiltinSink::finding_with_only(
        "sqlalchemy.engine.Engine.begin",
        RULE_BLOCKING_DB,
        "Use `AsyncEngine.begin()` / async SQLAlchemy, or offload. \
         (`Engine.begin` blocks when used directly in `with`.)",
    ),
    BuiltinSink::finding(
        "sqlalchemy.engine.Connection.execute",
        RULE_BLOCKING_DB,
        "Use async SQLAlchemy (`AsyncConnection`) or offload.",
    ),
    // --- psycopg / psycopg2 ---
    BuiltinSink::finding(
        "psycopg.connect",
        RULE_BLOCKING_DB,
        "Use `psycopg` async (`AsyncConnection`) / `asyncpg`, or offload.",
    ),
    BuiltinSink::finding(
        "psycopg2.connect",
        RULE_BLOCKING_DB,
        "Use `psycopg` async / `asyncpg`, or offload.",
    ),
    BuiltinSink::finding(
        "psycopg2.extensions.connection.cursor",
        RULE_BLOCKING_DB,
        "Use `psycopg` async / `asyncpg`, or offload.",
    ),
    BuiltinSink::finding(
        "psycopg.Connection.execute",
        RULE_BLOCKING_DB,
        "Use `psycopg.AsyncConnection`, or offload.",
    ),
    BuiltinSink::finding(
        "psycopg.Cursor.execute",
        RULE_BLOCKING_DB,
        "Use `psycopg.AsyncCursor`, or offload.",
    ),
    // --- pymysql ---
    BuiltinSink::finding(
        "pymysql.connect",
        RULE_BLOCKING_DB,
        "Use `aiomysql` / `asyncmy`, or offload.",
    ),
    BuiltinSink::finding(
        "pymysql.connections.Connection.cursor",
        RULE_BLOCKING_DB,
        "Use `aiomysql` / `asyncmy`, or offload.",
    ),
    BuiltinSink::finding(
        "pymysql.cursors.Cursor.execute",
        RULE_BLOCKING_DB,
        "Use `aiomysql` / `asyncmy`, or offload.",
    ),
    // --- pymongo ---
    BuiltinSink::finding(
        "pymongo.collection.Collection.find",
        RULE_BLOCKING_DB,
        "Use `motor` (async) instead of sync `pymongo`.",
    ),
    BuiltinSink::finding(
        "pymongo.collection.Collection.insert_one",
        RULE_BLOCKING_DB,
        "Use `motor` (async) instead of sync `pymongo`.",
    ),
    BuiltinSink::finding(
        "pymongo.mongo_client.MongoClient",
        RULE_BLOCKING_DB,
        "Use `motor.motor_asyncio.AsyncIOMotorClient` instead of sync `MongoClient`.",
    ),
    // --- boto3 / botocore ---
    BuiltinSink::finding(
        "botocore.client.BaseClient._make_api_call",
        RULE_BLOCKING_HTTP,
        "Use `aioboto3` / `aiobotocore` instead of sync `boto3`.",
    ),
    BuiltinSink::finding(
        "boto3.client",
        RULE_BLOCKING_HTTP,
        "Use `aioboto3` instead of sync `boto3.client`.",
    ),
    BuiltinSink::finding(
        "boto3.resource",
        RULE_BLOCKING_HTTP,
        "Use `aioboto3` instead of sync `boto3.resource`.",
    ),
    // --- openai / anthropic sync ---
    BuiltinSink::finding(
        "openai.OpenAI",
        RULE_BLOCKING_HTTP,
        "Use `openai.AsyncOpenAI` instead of sync `OpenAI`.",
    ),
    BuiltinSink::finding(
        "anthropic.Anthropic",
        RULE_BLOCKING_HTTP,
        "Use `anthropic.AsyncAnthropic` instead of sync `Anthropic`.",
    ),
    // --- kafka-python ---
    BuiltinSink::finding(
        "kafka.KafkaProducer.send",
        RULE_BLOCKING_HTTP,
        "Use `aiokafka` instead of sync `kafka-python`.",
    ),
    BuiltinSink::finding(
        "kafka.KafkaConsumer.poll",
        RULE_BLOCKING_HTTP,
        "Use `aiokafka` instead of sync `kafka-python`.",
    ),
    // --- elasticsearch sync ---
    BuiltinSink::finding(
        "elasticsearch.Elasticsearch.search",
        RULE_BLOCKING_HTTP,
        "Use `elasticsearch.AsyncElasticsearch` instead of sync client.",
    ),
    BuiltinSink::finding(
        "elasticsearch.Elasticsearch.index",
        RULE_BLOCKING_HTTP,
        "Use `elasticsearch.AsyncElasticsearch` instead of sync client.",
    ),
    // --- grpc sync ---
    BuiltinSink::finding(
        "grpc.Channel",
        RULE_BLOCKING_HTTP,
        "Use `grpc.aio` channel/stubs instead of sync gRPC.",
    ),
    BuiltinSink::finding(
        "grpc._channel._UnaryUnaryMultiCallable.__call__",
        RULE_BLOCKING_HTTP,
        "Use `grpc.aio` stubs instead of sync gRPC callables.",
    ),
];

const THIRD_PARTY_ALIASES: &[(&str, &str)] = &[
    ("Session.get", "requests.Session.get"),
    ("Session.post", "requests.Session.post"),
    ("Session.request", "requests.Session.request"),
    ("Client.get", "httpx.Client.get"),
    ("Client.post", "httpx.Client.post"),
    ("Client.request", "httpx.Client.request"),
    ("PoolManager.request", "urllib3.PoolManager.request"),
    ("Redis.get", "redis.Redis.get"),
    ("Redis.set", "redis.Redis.set"),
    ("Session.execute", "sqlalchemy.orm.Session.execute"),
    ("Engine.begin", "sqlalchemy.engine.Engine.begin"),
    ("Engine.connect", "sqlalchemy.engine.Engine.connect"),
    ("MongoClient", "pymongo.mongo_client.MongoClient"),
    ("KafkaProducer.send", "kafka.KafkaProducer.send"),
    ("Elasticsearch.search", "elasticsearch.Elasticsearch.search"),
];

#[cfg(test)]
mod tests {
    use super::*;
    use async_doctor_core::SinkTable;

    #[test]
    fn requests_matches_async_twin_hint() {
        let t = BuiltinSinkTable::new();
        let hit = t.lookup("requests.get").unwrap();
        assert_eq!(hit.rule, RULE_BLOCKING_HTTP);
        assert!(hit.fix_hint.contains("httpx.AsyncClient"));
    }

    #[test]
    fn httpx_async_client_never_fires() {
        let t = BuiltinSinkTable::new();
        assert!(t.lookup("httpx.AsyncClient.get").is_none());
        assert!(t.lookup("httpx.Client.get").is_some());
    }

    #[test]
    fn redis_asyncio_never_fires() {
        let t = BuiltinSinkTable::new();
        assert!(t.lookup("redis.asyncio.Redis.get").is_none());
        assert!(t.lookup("redis.Redis.get").is_some());
    }

    #[test]
    fn redis_pipeline_buffering_never_fires_execute_does() {
        let t = BuiltinSinkTable::new();
        // Receiver-type Pipeline keys (post receiver-preferring extraction).
        assert!(t.lookup("redis.client.Pipeline.set").is_none());
        assert!(t.lookup("redis.client.Pipeline.get").is_none());
        assert!(t.lookup("redis.client.Pipeline.delete").is_none());
        assert!(t.lookup("redis.Redis.pipeline").is_none());
        assert!(t.lookup("redis.Pipeline.set").is_none());
        assert!(t.lookup("redis.client.Pipeline.execute").is_some());
        assert!(t.lookup("redis.Pipeline.execute").is_some());
        assert!(t.lookup("redis.Redis.get").is_some());
        // Mixin defining-class alone still looks like Redis IO — extraction must
        // emit receiver-based keys instead. Async Pipeline.execute must never fire.
        assert!(t.lookup("redis.asyncio.client.Pipeline.execute").is_none());
        assert!(t.lookup("redis.asyncio.Pipeline.execute").is_none());
    }

    #[test]
    fn sqlalchemy_async_session_never_fires() {
        let t = BuiltinSinkTable::new();
        assert!(
            t.lookup("sqlalchemy.ext.asyncio.AsyncSession.execute")
                .is_none()
        );
        assert!(t.lookup("sqlalchemy.orm.Session.execute").is_some());
    }

    #[test]
    fn openai_async_never_fires() {
        let t = BuiltinSinkTable::new();
        assert!(
            t.lookup("openai.AsyncOpenAI.chat.completions.create")
                .is_none()
        );
        assert!(t.lookup("openai.OpenAI").is_some());
    }

    #[test]
    fn stdlib_still_resolves() {
        let t = BuiltinSinkTable::new();
        assert!(t.lookup("time.sleep").is_some());
    }
}
