//! Built-in third-party blocking sinks (type-based matching).
//!
//! Used both as a shortcut (skip following into known libraries) and for C
//! extensions without source. Async counterparts must never match.

mod families;
mod sinks;
#[cfg(test)]
mod tests;

use async_doctor_core::{AsyncTwinHit, SinkHit, SinkTable};

use crate::async_twin::async_twin_for_sink;
use crate::{BuiltinSink, StdlibSinkTable};

use families::{is_redis_pipeline_buffering, match_family};
use sinks::THIRD_PARTY_ALIASES;
pub use sinks::THIRD_PARTY_SINKS;

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

    fn async_twin(&self, qualname: &str) -> Option<AsyncTwinHit> {
        async_twin_for_sink(qualname)
    }
}

pub(super) fn hit(sink: &BuiltinSink) -> SinkHit {
    sink.hit()
}

pub fn is_async_counterpart(qualname: &str) -> bool {
    // Normalize stub/runtime layouts (`httpx._client.AsyncClient` → `httpx.AsyncClient`)
    // before matching — follow and lookup must agree, and definition-qualified
    // paths from ty must not slip past the public-type checks.
    let normalized = normalize_third_party(qualname);
    for q in [qualname, normalized.as_str()] {
        if is_async_counterpart_raw(q) {
            return true;
        }
    }
    false
}

fn is_async_counterpart_raw(q: &str) -> bool {
    // httpx — public type *or* any path segment `AsyncClient` under httpx.
    if q.starts_with("httpx.AsyncClient.")
        || q == "httpx.AsyncClient"
        || (q.starts_with("httpx.") && q.split('.').any(|s| s == "AsyncClient"))
    {
        return true;
    }
    // aiohttp
    if q.starts_with("aiohttp.") {
        return true;
    }
    // redis.asyncio
    if q.starts_with("redis.asyncio.") || (q.starts_with("redis.") && q.contains(".asyncio.")) {
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
        || q.contains("AsyncOpenAI")
        || q.starts_with("anthropic.AsyncAnthropic")
        || q.starts_with("anthropic.AsyncAnthropicBedrock")
        || q.contains("AsyncAnthropic")
    {
        return true;
    }
    // elasticsearch async — public type, `Async*` segment, or `_async` package path
    if q.starts_with("elasticsearch.AsyncElasticsearch")
        || q.starts_with("elasticsearch.Async")
        || q.starts_with("elasticsearch._async.")
        || (q.starts_with("elasticsearch.")
            && (q.contains("._async.") || q.split('.').any(|s| s.starts_with("Async"))))
    {
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
        || q.contains("AsyncConnection")
        || q.contains("AsyncCursor")
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
        // Never map mixin `redis.commands.core.CoreCommands.*` → `redis.Redis.*`:
        // that turns asyncio-pipeline / union-receiver fallbacks into false
        // sync Redis sinks. Receiver-typed Externals already key off Redis /
        // Pipeline; bare mixin quals are not sync-IO evidence.
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
