//! Explicit third-party sink rows and short-name aliases.

use crate::{BuiltinSink, RULE_BLOCKING_CPU, RULE_BLOCKING_DB, RULE_BLOCKING_HTTP};

/// Explicit third-party sink rows (plus family matchers).
pub static THIRD_PARTY_SINKS: &[BuiltinSink] = &[
    // --- requests ---
    BuiltinSink::finding("requests.request", RULE_BLOCKING_HTTP),
    BuiltinSink::finding("requests.get", RULE_BLOCKING_HTTP),
    BuiltinSink::finding("requests.post", RULE_BLOCKING_HTTP),
    BuiltinSink::finding("requests.put", RULE_BLOCKING_HTTP),
    BuiltinSink::finding("requests.patch", RULE_BLOCKING_HTTP),
    BuiltinSink::finding("requests.delete", RULE_BLOCKING_HTTP),
    BuiltinSink::finding("requests.head", RULE_BLOCKING_HTTP),
    BuiltinSink::finding("requests.Session.request", RULE_BLOCKING_HTTP),
    BuiltinSink::finding("requests.Session.get", RULE_BLOCKING_HTTP),
    BuiltinSink::finding("requests.Session.post", RULE_BLOCKING_HTTP),
    // --- httpx sync ---
    BuiltinSink::finding("httpx.Client.request", RULE_BLOCKING_HTTP),
    BuiltinSink::finding("httpx.Client.get", RULE_BLOCKING_HTTP),
    BuiltinSink::finding("httpx.Client.post", RULE_BLOCKING_HTTP),
    BuiltinSink::finding("httpx.Client.send", RULE_BLOCKING_HTTP),
    BuiltinSink::finding("httpx.get", RULE_BLOCKING_HTTP),
    BuiltinSink::finding("httpx.post", RULE_BLOCKING_HTTP),
    BuiltinSink::finding("httpx.put", RULE_BLOCKING_HTTP),
    BuiltinSink::finding("httpx.patch", RULE_BLOCKING_HTTP),
    BuiltinSink::finding("httpx.delete", RULE_BLOCKING_HTTP),
    BuiltinSink::finding("httpx.head", RULE_BLOCKING_HTTP),
    BuiltinSink::finding("httpx.options", RULE_BLOCKING_HTTP),
    BuiltinSink::finding("httpx.request", RULE_BLOCKING_HTTP),
    BuiltinSink::finding("httpx.stream", RULE_BLOCKING_HTTP),
    // --- urllib3 ---
    BuiltinSink::finding("urllib3.request", RULE_BLOCKING_HTTP),
    BuiltinSink::finding("urllib3.PoolManager.request", RULE_BLOCKING_HTTP),
    BuiltinSink::finding("urllib3.HTTPConnectionPool.urlopen", RULE_BLOCKING_HTTP),
    // --- redis / RedisCluster / valkey sync ---
    BuiltinSink::finding("redis.Redis.execute_command", RULE_BLOCKING_DB),
    BuiltinSink::finding("redis.Redis.get", RULE_BLOCKING_DB),
    BuiltinSink::finding("redis.Redis.set", RULE_BLOCKING_DB),
    BuiltinSink::finding("redis.Redis.delete", RULE_BLOCKING_DB),
    BuiltinSink::finding("redis.Redis.ping", RULE_BLOCKING_DB),
    BuiltinSink::finding(
        "redis.cluster.RedisCluster.execute_command",
        RULE_BLOCKING_DB,
    ),
    BuiltinSink::finding("redis.cluster.RedisCluster.get", RULE_BLOCKING_DB),
    BuiltinSink::finding("redis.cluster.RedisCluster.set", RULE_BLOCKING_DB),
    // Pipeline.execute is the only buffering→IO boundary.
    BuiltinSink::finding("redis.client.Pipeline.execute", RULE_BLOCKING_DB),
    BuiltinSink::finding("redis.cluster.ClusterPipeline.execute", RULE_BLOCKING_DB),
    // PubSub.listen blocks when the generator is consumed (for / list / next / comp).
    BuiltinSink::finding_consume_only("redis.PubSub.listen", RULE_BLOCKING_DB),
    BuiltinSink::finding("valkey.Valkey.execute_command", RULE_BLOCKING_DB),
    BuiltinSink::finding("valkey.Valkey.get", RULE_BLOCKING_DB),
    BuiltinSink::finding("valkey.Valkey.set", RULE_BLOCKING_DB),
    BuiltinSink::finding("valkey.client.Pipeline.execute", RULE_BLOCKING_DB),
    // --- sqlalchemy / sqlmodel sync (IO methods only; identity-map helpers are not sinks) ---
    BuiltinSink::finding("sqlalchemy.orm.Session.execute", RULE_BLOCKING_DB),
    BuiltinSink::finding("sqlalchemy.orm.Session.commit", RULE_BLOCKING_DB),
    BuiltinSink::finding("sqlalchemy.orm.Session.scalar", RULE_BLOCKING_DB),
    BuiltinSink::finding("sqlalchemy.orm.Session.scalars", RULE_BLOCKING_DB),
    BuiltinSink::finding("sqlalchemy.orm.Session.get", RULE_BLOCKING_DB),
    BuiltinSink::finding("sqlalchemy.orm.Session.flush", RULE_BLOCKING_DB),
    BuiltinSink::finding("sqlalchemy.orm.Session.rollback", RULE_BLOCKING_DB),
    BuiltinSink::finding("sqlalchemy.orm.Session.refresh", RULE_BLOCKING_DB),
    BuiltinSink::finding("sqlalchemy.orm.Session.connection", RULE_BLOCKING_DB),
    BuiltinSink::finding("sqlalchemy.orm.Session.merge", RULE_BLOCKING_DB),
    BuiltinSink::finding("sqlalchemy.orm.Session.close", RULE_BLOCKING_DB),
    BuiltinSink::finding("sqlalchemy.orm.Session.bulk_save_objects", RULE_BLOCKING_DB),
    BuiltinSink::finding(
        "sqlalchemy.orm.Session.bulk_insert_mappings",
        RULE_BLOCKING_DB,
    ),
    BuiltinSink::finding(
        "sqlalchemy.orm.Session.bulk_update_mappings",
        RULE_BLOCKING_DB,
    ),
    BuiltinSink::finding("sqlalchemy.orm.Query.__iter__", RULE_BLOCKING_DB),
    BuiltinSink::finding("sqlalchemy.engine.Engine.connect", RULE_BLOCKING_DB),
    BuiltinSink::finding_with_only("sqlalchemy.engine.Engine.begin", RULE_BLOCKING_DB),
    BuiltinSink::finding("sqlalchemy.engine.Connection.execute", RULE_BLOCKING_DB),
    BuiltinSink::finding("sqlmodel.Session.exec", RULE_BLOCKING_DB),
    // --- psycopg / psycopg2 ---
    BuiltinSink::finding("psycopg.connect", RULE_BLOCKING_DB),
    BuiltinSink::finding("psycopg2.connect", RULE_BLOCKING_DB),
    BuiltinSink::finding("psycopg2.extensions.connection.cursor", RULE_BLOCKING_DB),
    BuiltinSink::finding("psycopg.Connection.execute", RULE_BLOCKING_DB),
    BuiltinSink::finding("psycopg.Cursor.execute", RULE_BLOCKING_DB),
    // --- pymysql ---
    BuiltinSink::finding("pymysql.connect", RULE_BLOCKING_DB),
    BuiltinSink::finding("pymysql.connections.Connection.cursor", RULE_BLOCKING_DB),
    BuiltinSink::finding("pymysql.cursors.Cursor.execute", RULE_BLOCKING_DB),
    // --- pymongo ---
    BuiltinSink::finding("pymongo.collection.Collection.find", RULE_BLOCKING_DB),
    BuiltinSink::finding("pymongo.collection.Collection.insert_one", RULE_BLOCKING_DB),
    BuiltinSink::finding(
        "pymongo.collection.Collection.count_documents",
        RULE_BLOCKING_DB,
    ),
    BuiltinSink::finding(
        "pymongo.collection.Collection.estimated_document_count",
        RULE_BLOCKING_DB,
    ),
    BuiltinSink::finding("pymongo.collection.Collection.distinct", RULE_BLOCKING_DB),
    BuiltinSink::finding(
        "pymongo.collection.Collection.replace_one",
        RULE_BLOCKING_DB,
    ),
    BuiltinSink::finding("pymongo.collection.Collection.bulk_write", RULE_BLOCKING_DB),
    BuiltinSink::finding("pymongo.mongo_client.MongoClient", RULE_BLOCKING_DB),
    // --- boto3 / botocore ---
    BuiltinSink::finding(
        "botocore.client.BaseClient._make_api_call",
        RULE_BLOCKING_HTTP,
    ),
    BuiltinSink::finding("boto3.client", RULE_BLOCKING_HTTP),
    BuiltinSink::finding("boto3.resource", RULE_BLOCKING_HTTP),
    BuiltinSink::finding("botocore.waiter.Waiter.wait", RULE_BLOCKING_HTTP),
    BuiltinSink::finding("botocore.response.StreamingBody.read", RULE_BLOCKING_HTTP),
    BuiltinSink::finding(
        "botocore.response.StreamingBody.iter_lines",
        RULE_BLOCKING_HTTP,
    ),
    BuiltinSink::finding(
        "botocore.response.StreamingBody.iter_chunks",
        RULE_BLOCKING_HTTP,
    ),
    // --- AI / vector SDKs sync ---
    BuiltinSink::finding("openai.OpenAI", RULE_BLOCKING_HTTP),
    BuiltinSink::finding("anthropic.Anthropic", RULE_BLOCKING_HTTP),
    BuiltinSink::finding("anthropic.AnthropicBedrock", RULE_BLOCKING_HTTP),
    BuiltinSink::finding("anthropic.AnthropicVertex", RULE_BLOCKING_HTTP),
    BuiltinSink::finding("groq.Groq", RULE_BLOCKING_HTTP),
    BuiltinSink::finding("ollama.Client", RULE_BLOCKING_HTTP),
    BuiltinSink::finding("qdrant_client.QdrantClient", RULE_BLOCKING_HTTP),
    // --- kafka-python / confluent-kafka ---
    BuiltinSink::finding("kafka.KafkaProducer.send", RULE_BLOCKING_HTTP),
    BuiltinSink::finding("kafka.KafkaConsumer.poll", RULE_BLOCKING_HTTP),
    BuiltinSink::finding("confluent_kafka.Producer.flush", RULE_BLOCKING_HTTP),
    BuiltinSink::finding("confluent_kafka.Consumer.poll", RULE_BLOCKING_HTTP),
    BuiltinSink::finding("confluent_kafka.Consumer.consume", RULE_BLOCKING_HTTP),
    // --- elasticsearch sync ---
    BuiltinSink::finding("elasticsearch.Elasticsearch.search", RULE_BLOCKING_HTTP),
    BuiltinSink::finding("elasticsearch.Elasticsearch.index", RULE_BLOCKING_HTTP),
    // --- grpc sync ---
    BuiltinSink::finding("grpc.Channel", RULE_BLOCKING_HTTP),
    BuiltinSink::finding(
        "grpc._channel._UnaryUnaryMultiCallable.__call__",
        RULE_BLOCKING_HTTP,
    ),
    // --- slow password KDFs → strictly candidates (never findings) ---
    BuiltinSink::candidate("bcrypt.hashpw", RULE_BLOCKING_CPU, "slow-kdf"),
    BuiltinSink::candidate("bcrypt.checkpw", RULE_BLOCKING_CPU, "slow-kdf"),
    BuiltinSink::candidate("argon2.PasswordHasher.hash", RULE_BLOCKING_CPU, "slow-kdf"),
    BuiltinSink::candidate(
        "argon2.PasswordHasher.verify",
        RULE_BLOCKING_CPU,
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
