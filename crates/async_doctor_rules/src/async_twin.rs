//! Structured same-package async-API mappings and availability requirements.
//!
//! Resolved mappings support factual `offload-has-async-twin` advice at already-cut
//! offload sites, not blocking findings or migration recommendations. A mapped
//! API's availability does not establish a behavior-preserving substitution.

use async_doctor_core::{AsyncTwinHit, TwinConfidence};

/// One high-confidence same-package async twin mapping.
/// This describes an API relationship, not a recommended edit.
#[derive(Debug, Clone, Copy)]
pub struct AsyncTwinEntry {
    /// Human/symbol form, e.g. `httpx.AsyncClient`.
    pub symbol: &'static str,
    /// Extra modules that must ALL resolve in the scanned venv (e.g. `aiohttp`).
    pub needs: &'static [&'static str],
    /// Extra modules where AT LEAST ONE must resolve when non-empty (e.g. an async SQL driver).
    pub any_needs: &'static [&'static str],
    /// Confidence in the mapping, not in migration safety or suitability.
    pub confidence: TwinConfidence,
    /// Sink qualname prefixes / matchers that this twin covers.
    matchers: &'static [TwinMatcher],
}

#[derive(Debug, Clone, Copy)]
enum TwinMatcher {
    /// Sink qualname starts with this prefix (e.g. `httpx.Client.`).
    Prefix(&'static str),
    /// Exact sink qualname.
    Exact(&'static str),
}

const SQLALCHEMY_ASYNC_DRIVERS: &[&str] = &[
    "asyncpg",
    "aiosqlite",
    "aiomysql",
    "asyncmy",
    "psycopg.AsyncConnection",
];

/// Same-package mappings whose resolved availability can support `offload-has-async-twin`.
pub static ASYNC_TWINS: &[AsyncTwinEntry] = &[
    AsyncTwinEntry {
        symbol: "httpx.AsyncClient",
        needs: &[],
        any_needs: &[],
        confidence: TwinConfidence::High,
        matchers: &[
            TwinMatcher::Prefix("httpx.Client."),
            TwinMatcher::Prefix("httpx._client.Client."),
            TwinMatcher::Exact("httpx.get"),
            TwinMatcher::Exact("httpx.post"),
            TwinMatcher::Exact("httpx.put"),
            TwinMatcher::Exact("httpx.patch"),
            TwinMatcher::Exact("httpx.delete"),
            TwinMatcher::Exact("httpx.head"),
            TwinMatcher::Exact("httpx.options"),
            TwinMatcher::Exact("httpx.request"),
            TwinMatcher::Exact("httpx.stream"),
        ],
    },
    AsyncTwinEntry {
        symbol: "redis.asyncio",
        needs: &[],
        any_needs: &[],
        confidence: TwinConfidence::High,
        matchers: &[
            TwinMatcher::Prefix("redis.Redis."),
            TwinMatcher::Prefix("redis.StrictRedis."),
            TwinMatcher::Prefix("redis.client.Redis."),
            TwinMatcher::Prefix("redis.client.StrictRedis."),
            TwinMatcher::Prefix("redis.RedisCluster."),
            TwinMatcher::Prefix("redis.cluster.RedisCluster."),
            TwinMatcher::Prefix("redis.PubSub."),
            TwinMatcher::Prefix("redis.client.PubSub."),
            TwinMatcher::Prefix("redis.Lock."),
            TwinMatcher::Prefix("redis.lock.Lock."),
            TwinMatcher::Exact("redis.client.Pipeline.execute"),
            TwinMatcher::Prefix("redis.client.Pipeline.execute"),
            TwinMatcher::Prefix("redis.Pipeline.execute"),
            TwinMatcher::Prefix("redis.ClusterPipeline.execute"),
            TwinMatcher::Prefix("redis.cluster.ClusterPipeline.execute"),
        ],
    },
    AsyncTwinEntry {
        symbol: "valkey.asyncio.Valkey",
        needs: &[],
        any_needs: &[],
        confidence: TwinConfidence::High,
        matchers: &[
            TwinMatcher::Prefix("valkey.Valkey."),
            TwinMatcher::Prefix("valkey.StrictValkey."),
            TwinMatcher::Prefix("valkey.client.Valkey."),
            TwinMatcher::Prefix("valkey.client.StrictValkey."),
            TwinMatcher::Prefix("valkey.ValkeyCluster."),
            TwinMatcher::Prefix("valkey.cluster.ValkeyCluster."),
            TwinMatcher::Prefix("valkey.PubSub."),
            TwinMatcher::Prefix("valkey.client.PubSub."),
            TwinMatcher::Prefix("valkey.Lock."),
            TwinMatcher::Prefix("valkey.lock.Lock."),
            TwinMatcher::Prefix("valkey.Pipeline.execute"),
            TwinMatcher::Prefix("valkey.client.Pipeline.execute"),
            TwinMatcher::Prefix("valkey.ClusterPipeline.execute"),
            TwinMatcher::Prefix("valkey.cluster.ClusterPipeline.execute"),
        ],
    },
    // Prefer Azure twin before the generic OpenAI resources prefix.
    AsyncTwinEntry {
        symbol: "openai.AsyncAzureOpenAI",
        needs: &[],
        any_needs: &[],
        confidence: TwinConfidence::High,
        matchers: &[
            TwinMatcher::Prefix("openai.AzureOpenAI."),
            TwinMatcher::Exact("openai.AzureOpenAI"),
            TwinMatcher::Prefix("openai.lib.azure.AzureOpenAI."),
            TwinMatcher::Exact("openai.lib.azure.AzureOpenAI"),
        ],
    },
    AsyncTwinEntry {
        symbol: "openai.AsyncOpenAI",
        needs: &[],
        any_needs: &[],
        confidence: TwinConfidence::High,
        matchers: &[
            TwinMatcher::Prefix("openai.OpenAI."),
            TwinMatcher::Exact("openai.OpenAI"),
            TwinMatcher::Prefix("openai.resources."),
        ],
    },
    // Prefer Bedrock / Vertex twins before generic Anthropic.
    AsyncTwinEntry {
        symbol: "anthropic.AsyncAnthropicBedrock",
        needs: &[],
        any_needs: &[],
        confidence: TwinConfidence::High,
        matchers: &[
            TwinMatcher::Prefix("anthropic.AnthropicBedrock."),
            TwinMatcher::Exact("anthropic.AnthropicBedrock"),
            TwinMatcher::Prefix("anthropic.lib.bedrock.AnthropicBedrock."),
            TwinMatcher::Exact("anthropic.lib.bedrock.AnthropicBedrock"),
            TwinMatcher::Prefix("anthropic.lib.bedrock._client.AnthropicBedrock."),
            TwinMatcher::Exact("anthropic.lib.bedrock._client.AnthropicBedrock"),
        ],
    },
    AsyncTwinEntry {
        symbol: "anthropic.AsyncAnthropicVertex",
        needs: &[],
        any_needs: &[],
        confidence: TwinConfidence::High,
        matchers: &[
            TwinMatcher::Prefix("anthropic.AnthropicVertex."),
            TwinMatcher::Exact("anthropic.AnthropicVertex"),
            TwinMatcher::Prefix("anthropic.lib.vertex.AnthropicVertex."),
            TwinMatcher::Exact("anthropic.lib.vertex.AnthropicVertex"),
            TwinMatcher::Prefix("anthropic.lib.vertex._client.AnthropicVertex."),
            TwinMatcher::Exact("anthropic.lib.vertex._client.AnthropicVertex"),
        ],
    },
    AsyncTwinEntry {
        symbol: "anthropic.AsyncAnthropic",
        needs: &[],
        any_needs: &[],
        confidence: TwinConfidence::High,
        matchers: &[
            TwinMatcher::Prefix("anthropic.Anthropic."),
            TwinMatcher::Exact("anthropic.Anthropic"),
            TwinMatcher::Prefix("anthropic.resources."),
        ],
    },
    AsyncTwinEntry {
        symbol: "groq.AsyncGroq",
        needs: &[],
        any_needs: &[],
        confidence: TwinConfidence::High,
        matchers: &[
            TwinMatcher::Exact("groq.Groq"),
            TwinMatcher::Prefix("groq.Groq."),
            TwinMatcher::Exact("groq._client.Groq"),
            TwinMatcher::Prefix("groq._client.Groq."),
            TwinMatcher::Prefix("groq.resources."),
        ],
    },
    AsyncTwinEntry {
        symbol: "ollama.AsyncClient",
        needs: &[],
        any_needs: &[],
        confidence: TwinConfidence::High,
        matchers: &[
            TwinMatcher::Exact("ollama.Client"),
            TwinMatcher::Prefix("ollama.Client."),
            TwinMatcher::Exact("ollama._client.Client"),
            TwinMatcher::Prefix("ollama._client.Client."),
            TwinMatcher::Exact("ollama.chat"),
            TwinMatcher::Exact("ollama.generate"),
            TwinMatcher::Exact("ollama.embed"),
            TwinMatcher::Exact("ollama.embeddings"),
            TwinMatcher::Exact("ollama.list"),
            TwinMatcher::Exact("ollama.show"),
            TwinMatcher::Exact("ollama.pull"),
            TwinMatcher::Exact("ollama.push"),
            TwinMatcher::Exact("ollama.create"),
            TwinMatcher::Exact("ollama.delete"),
            TwinMatcher::Exact("ollama.copy"),
            TwinMatcher::Exact("ollama.ps"),
        ],
    },
    AsyncTwinEntry {
        symbol: "qdrant_client.AsyncQdrantClient",
        needs: &[],
        any_needs: &[],
        confidence: TwinConfidence::High,
        matchers: &[
            TwinMatcher::Exact("qdrant_client.QdrantClient"),
            TwinMatcher::Prefix("qdrant_client.QdrantClient."),
            TwinMatcher::Exact("qdrant_client.qdrant_client.QdrantClient"),
            TwinMatcher::Prefix("qdrant_client.qdrant_client.QdrantClient."),
        ],
    },
    AsyncTwinEntry {
        symbol: "elasticsearch.AsyncElasticsearch",
        needs: &["aiohttp"],
        any_needs: &[],
        confidence: TwinConfidence::High,
        matchers: &[
            TwinMatcher::Prefix("elasticsearch.Elasticsearch."),
            TwinMatcher::Exact("elasticsearch.Elasticsearch"),
        ],
    },
    AsyncTwinEntry {
        symbol: "psycopg.AsyncConnection",
        needs: &[],
        any_needs: &[],
        confidence: TwinConfidence::High,
        matchers: &[
            TwinMatcher::Exact("psycopg.connect"),
            TwinMatcher::Prefix("psycopg.Connection."),
            TwinMatcher::Exact("psycopg.Connection"),
            TwinMatcher::Prefix("psycopg.connection.Connection."),
            TwinMatcher::Prefix("psycopg.Cursor."),
            TwinMatcher::Prefix("psycopg.cursor.Cursor."),
        ],
    },
    AsyncTwinEntry {
        symbol: "grpc.aio",
        needs: &[],
        any_needs: &[],
        confidence: TwinConfidence::High,
        matchers: &[
            TwinMatcher::Exact("grpc.Channel"),
            TwinMatcher::Prefix("grpc.Channel."),
            TwinMatcher::Prefix("grpc._channel."),
            TwinMatcher::Exact("grpc.insecure_channel"),
            TwinMatcher::Exact("grpc.secure_channel"),
        ],
    },
    AsyncTwinEntry {
        symbol: "sqlalchemy.ext.asyncio.AsyncSession",
        needs: &["greenlet"],
        any_needs: SQLALCHEMY_ASYNC_DRIVERS,
        confidence: TwinConfidence::High,
        matchers: &[
            TwinMatcher::Prefix("sqlalchemy.orm.Session."),
            TwinMatcher::Prefix("sqlalchemy.orm.session.Session."),
            TwinMatcher::Prefix("sqlalchemy.orm.Query."),
            TwinMatcher::Prefix("sqlalchemy.orm.query.Query."),
        ],
    },
    AsyncTwinEntry {
        symbol: "sqlalchemy.ext.asyncio.AsyncEngine",
        needs: &["greenlet"],
        any_needs: SQLALCHEMY_ASYNC_DRIVERS,
        confidence: TwinConfidence::High,
        matchers: &[
            TwinMatcher::Prefix("sqlalchemy.engine.Engine."),
            TwinMatcher::Prefix("sqlalchemy.engine.base.Engine."),
            TwinMatcher::Prefix("sqlalchemy.engine.Connection."),
            TwinMatcher::Prefix("sqlalchemy.engine.base.Connection."),
        ],
    },
    AsyncTwinEntry {
        symbol: "sqlmodel.ext.asyncio.session.AsyncSession",
        needs: &["greenlet"],
        any_needs: SQLALCHEMY_ASYNC_DRIVERS,
        confidence: TwinConfidence::High,
        matchers: &[
            TwinMatcher::Prefix("sqlmodel.Session."),
            TwinMatcher::Prefix("sqlmodel.orm.session.Session."),
        ],
    },
];

/// Every symbol that may need resolution to establish mapped API availability.
pub fn twin_resolution_symbols() -> Vec<&'static str> {
    let mut out = Vec::new();
    for t in ASYNC_TWINS {
        out.push(t.symbol);
        out.extend(t.needs.iter().copied());
        out.extend(t.any_needs.iter().copied());
    }
    out.sort_unstable();
    out.dedup();
    out
}

/// Look up a high-confidence same-package twin for a sink qualname.
///
/// Callers resolve the returned symbol and dependency requirements in the scanned
/// environment. A match alone establishes neither availability nor a safe migration.
///
/// Returns `None` for excluded families (pymongo, requests, boto3, file IO, …)
/// and for sinks without a structured twin entry.
pub fn async_twin_for_sink(qualname: &str) -> Option<AsyncTwinHit> {
    if is_excluded_from_twin_advice(qualname) {
        return None;
    }
    for twin in ASYNC_TWINS {
        if twin.confidence != TwinConfidence::High {
            continue;
        }
        if twin.matchers.iter().any(|m| m.matches(qualname)) {
            if qualname.contains("Async") {
                return None;
            }
            return Some(AsyncTwinHit {
                symbol: twin.symbol,
                same_package: true,
                needs: twin.needs,
                any_needs: twin.any_needs,
                confidence: twin.confidence,
            });
        }
    }
    None
}

impl TwinMatcher {
    fn matches(self, qualname: &str) -> bool {
        match self {
            Self::Prefix(p) => qualname.starts_with(p),
            Self::Exact(e) => qualname == e,
        }
    }
}

/// Families that must never produce `offload-has-async-twin` advice.
pub fn is_excluded_from_twin_advice(qualname: &str) -> bool {
    let q = qualname;
    // Different-package or non-drop-in async drivers.
    if q.starts_with("pymongo.")
        || q.starts_with("requests.")
        || q.starts_with("sqlite3.")
        || q.starts_with("_sqlite3.")
        || q.starts_with("psycopg2.")
        || q.starts_with("confluent_kafka.")
        || q.starts_with("kafka.")
        || q.starts_with("http.client.")
        || crate::is_sync_boto_package_qual(q)
    {
        return true;
    }
    // File IO / pathlib / os filesystem.
    if q.starts_with("builtins.open")
        || q == "open"
        || q.starts_with("io.")
        || q.starts_with("_io.")
        || q.starts_with("pathlib.")
        || q.starts_with("os.")
            && (q.contains("open")
                || q.contains("read")
                || q.contains("write")
                || q.contains("fsync")
                || q.contains("fdatasync")
                || q.contains("stat")
                || q.contains("listdir")
                || q.contains("walk")
                || q.contains("mkdir")
                || q.contains("remove")
                || q.contains("unlink")
                || q.contains("rename")
                || q.contains("scandir"))
        || q.starts_with("shutil.")
    {
        return true;
    }
    // Subprocess / system.
    if q.starts_with("subprocess.") || q == "os.system" || q == "os.popen" || q == "os.waitpid" {
        return true;
    }
    // CPU / KDF / sleep / locks — not IO twins.
    if q.starts_with("time.sleep")
        || q.starts_with("time.sleep_ns")
        || q.starts_with("_thread.")
        || q.starts_with("threading.")
        || q.starts_with("queue.")
        || q.starts_with("concurrent.futures.Future.result")
        || q.starts_with("hashlib.")
        || q.starts_with("_hashlib.")
        || q.starts_with("bcrypt.")
        || q.starts_with("argon2.")
        || q.starts_with("hmac.")
        || q.starts_with("zlib.")
        || q.starts_with("bz2.")
        || q.starts_with("lzma.")
        || q.starts_with("json.loads")
        || q.starts_with("json.dumps")
        || q.starts_with("json.load")
        || q.starts_with("json.dump")
    {
        return true;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn httpx_client_maps_to_async_client() {
        let t = async_twin_for_sink("httpx.Client.get").unwrap();
        assert_eq!(t.symbol, "httpx.AsyncClient");
        assert!(t.needs.is_empty());
    }

    #[test]
    fn httpx_module_get_maps_to_async_client() {
        let t = async_twin_for_sink("httpx.get").unwrap();
        assert_eq!(t.symbol, "httpx.AsyncClient");
    }

    #[test]
    fn azure_openai_maps_to_async_azure() {
        let t = async_twin_for_sink("openai.AzureOpenAI.chat.completions.create").unwrap();
        assert_eq!(t.symbol, "openai.AsyncAzureOpenAI");
    }

    #[test]
    fn elasticsearch_needs_aiohttp() {
        let t = async_twin_for_sink("elasticsearch.Elasticsearch.search").unwrap();
        assert_eq!(t.symbol, "elasticsearch.AsyncElasticsearch");
        assert_eq!(t.needs, &["aiohttp"]);
    }

    #[test]
    fn excluded_families_never_match() {
        for q in [
            "pymongo.collection.Collection.find",
            "requests.get",
            "boto3.client",
            "builtins.open",
            "subprocess.run",
            "time.sleep",
            "psycopg2.connect",
            "sqlite3.Connection.execute",
            "confluent_kafka.Consumer.poll",
            "bcrypt.hashpw",
            "argon2.PasswordHasher.verify",
        ] {
            assert!(
                async_twin_for_sink(q).is_none(),
                "{q} must be excluded from twin advice"
            );
        }
    }

    #[test]
    fn redis_pipeline_execute_has_twin() {
        assert!(async_twin_for_sink("redis.client.Pipeline.execute").is_some());
        assert!(async_twin_for_sink("redis.Lock.acquire").is_some());
    }

    #[test]
    fn grpc_and_psycopg_connect_and_sqlalchemy_map_to_async_twins() {
        let grpc = async_twin_for_sink("grpc.Channel").unwrap();
        assert_eq!(grpc.symbol, "grpc.aio");
        assert!(grpc.needs.is_empty());

        let grpc_call =
            async_twin_for_sink("grpc._channel._UnaryUnaryMultiCallable.__call__").unwrap();
        assert_eq!(grpc_call.symbol, "grpc.aio");

        let pg = async_twin_for_sink("psycopg.connect").unwrap();
        assert_eq!(pg.symbol, "psycopg.AsyncConnection");

        let sqla_sess = async_twin_for_sink("sqlalchemy.orm.Session.execute").unwrap();
        assert_eq!(sqla_sess.symbol, "sqlalchemy.ext.asyncio.AsyncSession");
        assert_eq!(sqla_sess.needs, &["greenlet"]);
        assert!(sqla_sess.any_needs.contains(&"asyncpg"));
        assert!(sqla_sess.any_needs.contains(&"aiosqlite"));

        let sqla_eng = async_twin_for_sink("sqlalchemy.engine.Engine.connect").unwrap();
        assert_eq!(sqla_eng.symbol, "sqlalchemy.ext.asyncio.AsyncEngine");
        assert_eq!(sqla_eng.needs, &["greenlet"]);
    }
}
