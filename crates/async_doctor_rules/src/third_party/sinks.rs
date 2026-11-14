//! Explicit third-party sink rows and short-name aliases.

use crate::{BuiltinSink, RULE_BLOCKING_CPU, RULE_BLOCKING_DB, RULE_BLOCKING_HTTP};

const REQUESTS_HINT: &str = "Use `httpx.AsyncClient` (or `aiohttp`) instead of sync `requests`.";
const REQUESTS_SESSION_HINT: &str = "Use `httpx.AsyncClient` instead of sync `requests.Session`.";
const HTTPX_CLIENT_HINT: &str = "Use `httpx.AsyncClient` instead of sync `httpx.Client`.";
const HTTPX_MODULE_HINT: &str =
    "Use `httpx.AsyncClient` instead of sync module-level `httpx` calls.";
const URLLIB3_HINT: &str = "Use an async HTTP client (`httpx.AsyncClient`), or offload.";
const REDIS_HINT: &str = "Use `redis.asyncio.Redis` instead of sync `redis.Redis`.";
const REDIS_CLUSTER_HINT: &str =
    "Use `redis.asyncio.cluster.RedisCluster` instead of sync `RedisCluster`.";
const VALKEY_HINT: &str = "Use `valkey.asyncio.Valkey` instead of sync `valkey.Valkey`.";
const SQLALCHEMY_SESSION_HINT: &str = "Use `sqlalchemy.ext.asyncio.AsyncSession`, or offload a self-contained sync unit-of-work function that opens and closes its own `Session` inside the worker thread.";
const SQLMODEL_SESSION_HINT: &str = "Use `sqlmodel.ext.asyncio.session.AsyncSession`, or offload a self-contained sync unit-of-work function that opens and closes its own `Session` inside the worker thread.";
const PSYCOPG_HINT: &str = "Use `psycopg.AsyncConnection` / `asyncpg`, or offload a self-contained sync function that opens and closes its own connection in the worker thread.";
const PYMYSQL_HINT: &str = "Use `aiomysql` / `asyncmy`, or offload a self-contained sync function that opens and closes its own connection in the worker thread.";
const PYMONGO_HINT: &str = "Use `motor` or `pymongo.AsyncMongoClient` instead of sync `pymongo`.";
const BOTO_HINT: &str = "Use `aioboto3` / `aiobotocore` instead of sync `boto3`.";

/// Explicit third-party sink rows (plus family matchers).
pub static THIRD_PARTY_SINKS: &[BuiltinSink] = &[
    // --- requests ---
    BuiltinSink::finding("requests.request", RULE_BLOCKING_HTTP, REQUESTS_HINT),
    BuiltinSink::finding("requests.get", RULE_BLOCKING_HTTP, REQUESTS_HINT),
    BuiltinSink::finding("requests.post", RULE_BLOCKING_HTTP, REQUESTS_HINT),
    BuiltinSink::finding("requests.put", RULE_BLOCKING_HTTP, REQUESTS_HINT),
    BuiltinSink::finding("requests.patch", RULE_BLOCKING_HTTP, REQUESTS_HINT),
    BuiltinSink::finding("requests.delete", RULE_BLOCKING_HTTP, REQUESTS_HINT),
    BuiltinSink::finding("requests.head", RULE_BLOCKING_HTTP, REQUESTS_HINT),
    BuiltinSink::finding(
        "requests.Session.request",
        RULE_BLOCKING_HTTP,
        REQUESTS_SESSION_HINT,
    ),
    BuiltinSink::finding(
        "requests.Session.get",
        RULE_BLOCKING_HTTP,
        REQUESTS_SESSION_HINT,
    ),
    BuiltinSink::finding(
        "requests.Session.post",
        RULE_BLOCKING_HTTP,
        REQUESTS_SESSION_HINT,
    ),
    // --- httpx sync ---
    BuiltinSink::finding(
        "httpx.Client.request",
        RULE_BLOCKING_HTTP,
        HTTPX_CLIENT_HINT,
    ),
    BuiltinSink::finding("httpx.Client.get", RULE_BLOCKING_HTTP, HTTPX_CLIENT_HINT),
    BuiltinSink::finding("httpx.Client.post", RULE_BLOCKING_HTTP, HTTPX_CLIENT_HINT),
    BuiltinSink::finding("httpx.Client.send", RULE_BLOCKING_HTTP, HTTPX_CLIENT_HINT),
    BuiltinSink::finding("httpx.get", RULE_BLOCKING_HTTP, HTTPX_MODULE_HINT),
    BuiltinSink::finding("httpx.post", RULE_BLOCKING_HTTP, HTTPX_MODULE_HINT),
    BuiltinSink::finding("httpx.put", RULE_BLOCKING_HTTP, HTTPX_MODULE_HINT),
    BuiltinSink::finding("httpx.patch", RULE_BLOCKING_HTTP, HTTPX_MODULE_HINT),
    BuiltinSink::finding("httpx.delete", RULE_BLOCKING_HTTP, HTTPX_MODULE_HINT),
    BuiltinSink::finding("httpx.head", RULE_BLOCKING_HTTP, HTTPX_MODULE_HINT),
    BuiltinSink::finding("httpx.options", RULE_BLOCKING_HTTP, HTTPX_MODULE_HINT),
    BuiltinSink::finding("httpx.request", RULE_BLOCKING_HTTP, HTTPX_MODULE_HINT),
    BuiltinSink::finding("httpx.stream", RULE_BLOCKING_HTTP, HTTPX_MODULE_HINT),
    // --- urllib3 ---
    BuiltinSink::finding("urllib3.request", RULE_BLOCKING_HTTP, URLLIB3_HINT),
    BuiltinSink::finding(
        "urllib3.PoolManager.request",
        RULE_BLOCKING_HTTP,
        URLLIB3_HINT,
    ),
    BuiltinSink::finding(
        "urllib3.HTTPConnectionPool.urlopen",
        RULE_BLOCKING_HTTP,
        URLLIB3_HINT,
    ),
    // --- redis / RedisCluster / valkey sync ---
    BuiltinSink::finding("redis.Redis.execute_command", RULE_BLOCKING_DB, REDIS_HINT),
    BuiltinSink::finding("redis.Redis.get", RULE_BLOCKING_DB, REDIS_HINT),
    BuiltinSink::finding("redis.Redis.set", RULE_BLOCKING_DB, REDIS_HINT),
    BuiltinSink::finding("redis.Redis.delete", RULE_BLOCKING_DB, REDIS_HINT),
    BuiltinSink::finding("redis.Redis.ping", RULE_BLOCKING_DB, REDIS_HINT),
    BuiltinSink::finding(
        "redis.cluster.RedisCluster.execute_command",
        RULE_BLOCKING_DB,
        REDIS_CLUSTER_HINT,
    ),
    BuiltinSink::finding(
        "redis.cluster.RedisCluster.get",
        RULE_BLOCKING_DB,
        REDIS_CLUSTER_HINT,
    ),
    BuiltinSink::finding(
        "redis.cluster.RedisCluster.set",
        RULE_BLOCKING_DB,
        REDIS_CLUSTER_HINT,
    ),
    // Pipeline.execute is the only buffering→IO boundary.
    BuiltinSink::finding(
        "redis.client.Pipeline.execute",
        RULE_BLOCKING_DB,
        "Use `redis.asyncio` pipelines (`await pipe.execute()`), or offload.",
    ),
    BuiltinSink::finding(
        "redis.cluster.ClusterPipeline.execute",
        RULE_BLOCKING_DB,
        "Use `redis.asyncio.cluster` pipelines (`await pipe.execute()`), or offload.",
    ),
    // PubSub.listen blocks when the generator is consumed (for / list / next / comp).
    BuiltinSink::finding_consume_only(
        "redis.PubSub.listen",
        RULE_BLOCKING_DB,
        "Use `redis.asyncio` pub/sub (`async for msg in pubsub.listen()`), or offload.",
    ),
    BuiltinSink::finding(
        "valkey.Valkey.execute_command",
        RULE_BLOCKING_DB,
        VALKEY_HINT,
    ),
    BuiltinSink::finding("valkey.Valkey.get", RULE_BLOCKING_DB, VALKEY_HINT),
    BuiltinSink::finding("valkey.Valkey.set", RULE_BLOCKING_DB, VALKEY_HINT),
    BuiltinSink::finding(
        "valkey.client.Pipeline.execute",
        RULE_BLOCKING_DB,
        "Use `valkey.asyncio` pipelines (`await pipe.execute()`), or offload.",
    ),
    // --- sqlalchemy / sqlmodel sync (IO methods only; identity-map helpers are not sinks) ---
    BuiltinSink::finding(
        "sqlalchemy.orm.Session.execute",
        RULE_BLOCKING_DB,
        SQLALCHEMY_SESSION_HINT,
    ),
    BuiltinSink::finding(
        "sqlalchemy.orm.Session.commit",
        RULE_BLOCKING_DB,
        SQLALCHEMY_SESSION_HINT,
    ),
    BuiltinSink::finding(
        "sqlalchemy.orm.Session.scalar",
        RULE_BLOCKING_DB,
        SQLALCHEMY_SESSION_HINT,
    ),
    BuiltinSink::finding(
        "sqlalchemy.orm.Session.scalars",
        RULE_BLOCKING_DB,
        SQLALCHEMY_SESSION_HINT,
    ),
    BuiltinSink::finding(
        "sqlalchemy.orm.Session.get",
        RULE_BLOCKING_DB,
        SQLALCHEMY_SESSION_HINT,
    ),
    BuiltinSink::finding(
        "sqlalchemy.orm.Session.flush",
        RULE_BLOCKING_DB,
        SQLALCHEMY_SESSION_HINT,
    ),
    BuiltinSink::finding(
        "sqlalchemy.orm.Session.rollback",
        RULE_BLOCKING_DB,
        SQLALCHEMY_SESSION_HINT,
    ),
    BuiltinSink::finding(
        "sqlalchemy.orm.Session.refresh",
        RULE_BLOCKING_DB,
        SQLALCHEMY_SESSION_HINT,
    ),
    BuiltinSink::finding(
        "sqlalchemy.orm.Session.connection",
        RULE_BLOCKING_DB,
        SQLALCHEMY_SESSION_HINT,
    ),
    BuiltinSink::finding(
        "sqlalchemy.orm.Session.merge",
        RULE_BLOCKING_DB,
        SQLALCHEMY_SESSION_HINT,
    ),
    BuiltinSink::finding(
        "sqlalchemy.orm.Session.close",
        RULE_BLOCKING_DB,
        SQLALCHEMY_SESSION_HINT,
    ),
    BuiltinSink::finding(
        "sqlalchemy.orm.Session.bulk_save_objects",
        RULE_BLOCKING_DB,
        SQLALCHEMY_SESSION_HINT,
    ),
    BuiltinSink::finding(
        "sqlalchemy.orm.Session.bulk_insert_mappings",
        RULE_BLOCKING_DB,
        SQLALCHEMY_SESSION_HINT,
    ),
    BuiltinSink::finding(
        "sqlalchemy.orm.Session.bulk_update_mappings",
        RULE_BLOCKING_DB,
        SQLALCHEMY_SESSION_HINT,
    ),
    BuiltinSink::finding(
        "sqlalchemy.orm.Query.__iter__",
        RULE_BLOCKING_DB,
        "Use `AsyncSession` / `await session.scalars(...)`, or offload a self-contained sync unit-of-work function.",
    ),
    BuiltinSink::finding(
        "sqlalchemy.engine.Engine.connect",
        RULE_BLOCKING_DB,
        "Use `sqlalchemy.ext.asyncio.create_async_engine` / `AsyncEngine`.",
    ),
    BuiltinSink::finding_with_only(
        "sqlalchemy.engine.Engine.begin",
        RULE_BLOCKING_DB,
        "Use `AsyncEngine.begin()` / async SQLAlchemy, or offload a self-contained sync unit-of-work function. (`Engine.begin` blocks when used directly in `with`.)",
    ),
    BuiltinSink::finding(
        "sqlalchemy.engine.Connection.execute",
        RULE_BLOCKING_DB,
        "Use async SQLAlchemy (`AsyncConnection`) or offload a self-contained sync unit-of-work function.",
    ),
    BuiltinSink::finding(
        "sqlmodel.Session.exec",
        RULE_BLOCKING_DB,
        SQLMODEL_SESSION_HINT,
    ),
    // --- psycopg / psycopg2 ---
    BuiltinSink::finding("psycopg.connect", RULE_BLOCKING_DB, PSYCOPG_HINT),
    BuiltinSink::finding("psycopg2.connect", RULE_BLOCKING_DB, PSYCOPG_HINT),
    BuiltinSink::finding(
        "psycopg2.extensions.connection.cursor",
        RULE_BLOCKING_DB,
        PSYCOPG_HINT,
    ),
    BuiltinSink::finding("psycopg.Connection.execute", RULE_BLOCKING_DB, PSYCOPG_HINT),
    BuiltinSink::finding("psycopg.Cursor.execute", RULE_BLOCKING_DB, PSYCOPG_HINT),
    // --- pymysql ---
    BuiltinSink::finding("pymysql.connect", RULE_BLOCKING_DB, PYMYSQL_HINT),
    BuiltinSink::finding(
        "pymysql.connections.Connection.cursor",
        RULE_BLOCKING_DB,
        PYMYSQL_HINT,
    ),
    BuiltinSink::finding(
        "pymysql.cursors.Cursor.execute",
        RULE_BLOCKING_DB,
        PYMYSQL_HINT,
    ),
    // --- pymongo ---
    BuiltinSink::finding(
        "pymongo.collection.Collection.find",
        RULE_BLOCKING_DB,
        PYMONGO_HINT,
    ),
    BuiltinSink::finding(
        "pymongo.collection.Collection.insert_one",
        RULE_BLOCKING_DB,
        PYMONGO_HINT,
    ),
    BuiltinSink::finding(
        "pymongo.collection.Collection.count_documents",
        RULE_BLOCKING_DB,
        PYMONGO_HINT,
    ),
    BuiltinSink::finding(
        "pymongo.collection.Collection.estimated_document_count",
        RULE_BLOCKING_DB,
        PYMONGO_HINT,
    ),
    BuiltinSink::finding(
        "pymongo.collection.Collection.distinct",
        RULE_BLOCKING_DB,
        PYMONGO_HINT,
    ),
    BuiltinSink::finding(
        "pymongo.collection.Collection.replace_one",
        RULE_BLOCKING_DB,
        PYMONGO_HINT,
    ),
    BuiltinSink::finding(
        "pymongo.collection.Collection.bulk_write",
        RULE_BLOCKING_DB,
        PYMONGO_HINT,
    ),
    BuiltinSink::finding(
        "pymongo.mongo_client.MongoClient",
        RULE_BLOCKING_DB,
        "Use `motor.motor_asyncio.AsyncIOMotorClient` or `pymongo.AsyncMongoClient` instead of sync `MongoClient`.",
    ),
    // --- boto3 / botocore ---
    BuiltinSink::finding(
        "botocore.client.BaseClient._make_api_call",
        RULE_BLOCKING_HTTP,
        BOTO_HINT,
    ),
    BuiltinSink::finding("boto3.client", RULE_BLOCKING_HTTP, BOTO_HINT),
    BuiltinSink::finding("boto3.resource", RULE_BLOCKING_HTTP, BOTO_HINT),
    BuiltinSink::finding(
        "botocore.waiter.Waiter.wait",
        RULE_BLOCKING_HTTP,
        "Use `aioboto3` / `aiobotocore` waiters (`await waiter.wait(...)`), or offload.",
    ),
    BuiltinSink::finding(
        "botocore.response.StreamingBody.read",
        RULE_BLOCKING_HTTP,
        "Use `aiobotocore` async response streams (`await body.read()`), or offload.",
    ),
    BuiltinSink::finding(
        "botocore.response.StreamingBody.iter_lines",
        RULE_BLOCKING_HTTP,
        "Use `aiobotocore` async response streams (`async for line in body.iter_lines()`), or offload.",
    ),
    BuiltinSink::finding(
        "botocore.response.StreamingBody.iter_chunks",
        RULE_BLOCKING_HTTP,
        "Use `aiobotocore` async response streams (`async for chunk in body.iter_chunks()`), or offload.",
    ),
    // --- AI / vector SDKs sync ---
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
    BuiltinSink::finding(
        "anthropic.AnthropicBedrock",
        RULE_BLOCKING_HTTP,
        "Use `anthropic.AsyncAnthropicBedrock` instead of sync `AnthropicBedrock`.",
    ),
    BuiltinSink::finding(
        "anthropic.AnthropicVertex",
        RULE_BLOCKING_HTTP,
        "Use `anthropic.AsyncAnthropicVertex` instead of sync `AnthropicVertex`.",
    ),
    BuiltinSink::finding(
        "groq.Groq",
        RULE_BLOCKING_HTTP,
        "Use `groq.AsyncGroq` instead of sync `Groq`.",
    ),
    BuiltinSink::finding(
        "ollama.Client",
        RULE_BLOCKING_HTTP,
        "Use `ollama.AsyncClient` instead of sync `ollama.Client`.",
    ),
    BuiltinSink::finding(
        "qdrant_client.QdrantClient",
        RULE_BLOCKING_HTTP,
        "Use `qdrant_client.AsyncQdrantClient` instead of sync `QdrantClient`.",
    ),
    // --- kafka-python / confluent-kafka ---
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
    BuiltinSink::finding(
        "confluent_kafka.Producer.flush",
        RULE_BLOCKING_HTTP,
        "Use `aiokafka` / `confluent_kafka.experimental.aio.AIOProducer`, or offload `Producer.flush`.",
    ),
    BuiltinSink::finding(
        "confluent_kafka.Consumer.poll",
        RULE_BLOCKING_HTTP,
        "Use `aiokafka` / `confluent_kafka.experimental.aio.AIOConsumer`, or offload `Consumer.poll`.",
    ),
    BuiltinSink::finding(
        "confluent_kafka.Consumer.consume",
        RULE_BLOCKING_HTTP,
        "Use `aiokafka` / `confluent_kafka.experimental.aio.AIOConsumer`, or offload `Consumer.consume`.",
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
    // --- slow password KDFs → strictly candidates (never findings) ---
    BuiltinSink::candidate(
        "bcrypt.hashpw",
        RULE_BLOCKING_CPU,
        "Slow password KDF; offload with `await asyncio.to_thread(bcrypt.hashpw, ...)`.",
        "slow-kdf",
    ),
    BuiltinSink::candidate(
        "bcrypt.checkpw",
        RULE_BLOCKING_CPU,
        "Slow password KDF; offload with `await asyncio.to_thread(bcrypt.checkpw, ...)`.",
        "slow-kdf",
    ),
    BuiltinSink::candidate(
        "argon2.PasswordHasher.hash",
        RULE_BLOCKING_CPU,
        "Slow password KDF; offload with `await asyncio.to_thread(ph.hash, ...)`.",
        "slow-kdf",
    ),
    BuiltinSink::candidate(
        "argon2.PasswordHasher.verify",
        RULE_BLOCKING_CPU,
        "Slow password KDF; offload with `await asyncio.to_thread(ph.verify, ...)`.",
        "slow-kdf",
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
    ("RedisCluster.get", "redis.cluster.RedisCluster.get"),
    ("RedisCluster.set", "redis.cluster.RedisCluster.set"),
    ("Valkey.get", "valkey.Valkey.get"),
    ("Valkey.set", "valkey.Valkey.set"),
    ("Session.execute", "sqlalchemy.orm.Session.execute"),
    ("Session.exec", "sqlmodel.Session.exec"),
    ("Engine.begin", "sqlalchemy.engine.Engine.begin"),
    ("Engine.connect", "sqlalchemy.engine.Engine.connect"),
    ("MongoClient", "pymongo.mongo_client.MongoClient"),
    ("KafkaProducer.send", "kafka.KafkaProducer.send"),
    ("Elasticsearch.search", "elasticsearch.Elasticsearch.search"),
    ("PasswordHasher.hash", "argon2.PasswordHasher.hash"),
    ("PasswordHasher.verify", "argon2.PasswordHasher.verify"),
];
