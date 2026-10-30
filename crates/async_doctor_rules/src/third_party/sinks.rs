//! Explicit third-party sink rows and short-name aliases.

use crate::{BuiltinSink, RULE_BLOCKING_DB, RULE_BLOCKING_HTTP};

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
    // PubSub.listen blocks when the generator is consumed (for / list / next / comp).
    BuiltinSink::finding_consume_only(
        "redis.PubSub.listen",
        RULE_BLOCKING_DB,
        "Use `redis.asyncio` pub/sub (`async for msg in pubsub.listen()`), or offload.",
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

pub(super) const THIRD_PARTY_ALIASES: &[(&str, &str)] = &[
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
