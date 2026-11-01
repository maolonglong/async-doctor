//! Structured async-twin table for high-confidence offload advice.
//!
//! Design exception (see `docs/decisions/2026-10-31-advice-tier.md`): twins
//! appear as a dedicated advice rule at offload cut sites — not as findings,
//! and not double-reported with blocking-* findings (those sites are already
//! cut).

use async_doctor_core::{AsyncTwinHit, TwinConfidence};

/// One high-confidence same-package async twin mapping.
#[derive(Debug, Clone, Copy)]
pub struct AsyncTwinEntry {
    /// Human/symbol form, e.g. `httpx.AsyncClient`.
    pub symbol: &'static str,
    /// Extra modules that must resolve in the scanned venv (e.g. `aiohttp`).
    pub needs: &'static [&'static str],
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

/// High-confidence same-package twins eligible for `offload-has-async-twin`.
pub static ASYNC_TWINS: &[AsyncTwinEntry] = &[
    AsyncTwinEntry {
        symbol: "httpx.AsyncClient",
        needs: &[],
        confidence: TwinConfidence::High,
        matchers: &[
            TwinMatcher::Prefix("httpx.Client."),
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
        confidence: TwinConfidence::High,
        matchers: &[
            TwinMatcher::Prefix("redis.Redis."),
            TwinMatcher::Prefix("redis.StrictRedis."),
            TwinMatcher::Prefix("redis.client.Redis."),
            TwinMatcher::Prefix("redis.client.StrictRedis."),
            TwinMatcher::Prefix("redis.PubSub."),
            TwinMatcher::Prefix("redis.client.PubSub."),
            TwinMatcher::Exact("redis.client.Pipeline.execute"),
            TwinMatcher::Prefix("redis.client.Pipeline.execute"),
        ],
    },
    // Prefer Azure twin before the generic OpenAI resources prefix.
    AsyncTwinEntry {
        symbol: "openai.AsyncAzureOpenAI",
        needs: &[],
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
        confidence: TwinConfidence::High,
        matchers: &[
            TwinMatcher::Prefix("openai.OpenAI."),
            TwinMatcher::Exact("openai.OpenAI"),
            TwinMatcher::Prefix("openai.resources."),
        ],
    },
    AsyncTwinEntry {
        symbol: "anthropic.AsyncAnthropic",
        needs: &[],
        confidence: TwinConfidence::High,
        matchers: &[
            TwinMatcher::Prefix("anthropic.Anthropic."),
            TwinMatcher::Exact("anthropic.Anthropic"),
            TwinMatcher::Prefix("anthropic.resources."),
        ],
    },
    AsyncTwinEntry {
        symbol: "elasticsearch.AsyncElasticsearch",
        needs: &["aiohttp"],
        confidence: TwinConfidence::High,
        matchers: &[
            TwinMatcher::Prefix("elasticsearch.Elasticsearch."),
            TwinMatcher::Exact("elasticsearch.Elasticsearch"),
        ],
    },
    AsyncTwinEntry {
        symbol: "psycopg.AsyncConnection",
        needs: &[],
        confidence: TwinConfidence::High,
        matchers: &[
            TwinMatcher::Prefix("psycopg.Connection."),
            TwinMatcher::Exact("psycopg.Connection"),
            TwinMatcher::Prefix("psycopg.connection.Connection."),
            TwinMatcher::Prefix("psycopg.Cursor."),
            TwinMatcher::Prefix("psycopg.cursor.Cursor."),
        ],
    },
];

/// Every symbol that advice may need to resolve in the scanned venv.
pub fn twin_resolution_symbols() -> Vec<&'static str> {
    let mut out = Vec::new();
    for t in ASYNC_TWINS {
        out.push(t.symbol);
        out.extend(t.needs.iter().copied());
    }
    out.sort_unstable();
    out.dedup();
    out
}

/// Look up a high-confidence same-package twin for a sink qualname.
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
            // openai.resources / anthropic.resources may include Async* paths.
            if qualname.contains("Async") {
                return None;
            }
            return Some(AsyncTwinHit {
                symbol: twin.symbol,
                same_package: true,
                needs: twin.needs,
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
    // pymongo — motor is a different package (not same-package twin).
    if q.starts_with("pymongo.") {
        return true;
    }
    // requests — twin is httpx/aiohttp (different package).
    if q.starts_with("requests.") {
        return true;
    }
    // boto3 / botocore — aioboto3 is a different package.
    if crate::is_sync_boto_package_qual(q) {
        return true;
    }
    // File IO / pathlib / os filesystem.
    if q.starts_with("builtins.open")
        || q == "open"
        || q.starts_with("io.")
        || q.starts_with("pathlib.")
        || q.starts_with("os.")
            && (q.contains("open")
                || q.contains("read")
                || q.contains("write")
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
    if q.starts_with("subprocess.") || q == "os.system" || q == "os.popen" {
        return true;
    }
    // CPU / sleep / locks — not IO twins.
    if q.starts_with("time.sleep")
        || q.starts_with("time.sleep_ns")
        || q.starts_with("_thread.")
        || q.starts_with("threading.")
        || q.starts_with("queue.")
        || q.starts_with("concurrent.futures.Future.result")
        || q.starts_with("hashlib.")
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
    // psycopg2 is sync-only (different from psycopg 3).
    if q.starts_with("psycopg2.") {
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
    }
}
