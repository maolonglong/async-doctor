//! Sync HTTP, RPC, messaging, AI, and vector-search SDK family matchers.

use async_doctor_core::SinkHit;

use crate::third_party::hit;
use crate::{BuiltinSink, RULE_BLOCKING_HTTP};

pub(super) fn match_http_ai_family(q: &str) -> Option<SinkHit> {
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
        )));
    }

    // httpx sync Client + module-level verbs/stream. `build_request` / `mount` do no IO.
    if q.starts_with("httpx.Client.")
        && !matches!(q, "httpx.Client.build_request" | "httpx.Client.mount")
    {
        return Some(hit(&BuiltinSink::finding(
            "httpx.Client.request",
            RULE_BLOCKING_HTTP,
        )));
    }
    if matches!(
        q,
        "httpx.get"
            | "httpx.post"
            | "httpx.put"
            | "httpx.patch"
            | "httpx.delete"
            | "httpx.head"
            | "httpx.options"
            | "httpx.request"
            | "httpx.stream"
    ) {
        return Some(hit(&BuiltinSink::finding(
            "httpx.request",
            RULE_BLOCKING_HTTP,
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
        )));
    }

    if let Some(res) = match_ai_or_vector_sdk(q) {
        return res;
    }

    // kafka-python. Consumer local-state mutators and metrics snapshots do no broker RPC.
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
        )));
    }

    // confluent_kafka — `Producer.produce` only enqueues to the local C buffer;
    // `Producer.flush` and `Consumer.{poll, consume}` block on broker IO.
    if q.starts_with("confluent_kafka.") && !q.contains(".aio.") && !q.contains("AIO") {
        if matches!(
            q,
            "confluent_kafka.Producer.flush" | "confluent_kafka.SerializingProducer.flush"
        ) {
            return Some(hit(&BuiltinSink::finding(
                "confluent_kafka.Producer.flush",
                RULE_BLOCKING_HTTP,
            )));
        }
        if matches!(
            q,
            "confluent_kafka.Consumer.poll"
                | "confluent_kafka.DeserializingConsumer.poll"
                | "confluent_kafka.Consumer.consume"
                | "confluent_kafka.DeserializingConsumer.consume"
        ) {
            return Some(hit(&BuiltinSink::finding(
                "confluent_kafka.Consumer.poll",
                RULE_BLOCKING_HTTP,
            )));
        }
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
        )));
    }

    // grpc sync stubs — exclude credentials, enums, and `.future(...)` (which
    // schedules an async RPC and returns a `grpc.Future` without blocking).
    if q.starts_with("grpc.")
        && !q.contains("aio")
        && !is_grpc_non_io_helper(q)
        && (q.contains("MultiCallable")
            || q.ends_with(".__call__")
            || q.ends_with(".with_call")
            || q.split('.').any(|seg| seg == "Channel")
            || q.contains("insecure_channel")
            || q.contains("secure_channel"))
    {
        return Some(hit(&BuiltinSink::finding(
            "grpc.Channel",
            RULE_BLOCKING_HTTP,
        )));
    }

    None
}

/// Returns `Some(Some(hit))` for sync AI/vector SDK calls, `Some(None)` for
/// pure client-copy helpers (`with_options` / `copy`), or `None` if unrelated.
fn match_ai_or_vector_sdk(q: &str) -> Option<Option<SinkHit>> {
    if is_ai_client_copy_helper(q) {
        return Some(None);
    }
    if q.contains("Async") {
        return None;
    }

    // OpenAI / AzureOpenAI
    if q.starts_with("openai.AzureOpenAI.") || q.starts_with("openai.lib.azure.AzureOpenAI.") {
        return Some(Some(hit(&BuiltinSink::finding(
            "openai.AzureOpenAI",
            RULE_BLOCKING_HTTP,
        ))));
    }
    if q.starts_with("openai.OpenAI.")
        || (q.starts_with("openai.resources.") && !q.split('.').any(|seg| seg.starts_with('_')))
    {
        return Some(Some(hit(&BuiltinSink::finding(
            "openai.OpenAI",
            RULE_BLOCKING_HTTP,
        ))));
    }

    // Anthropic cloud variants (check before generic Anthropic)
    if q == "anthropic.AnthropicBedrock"
        || q.starts_with("anthropic.AnthropicBedrock.")
        || (q.starts_with("anthropic.lib.bedrock.")
            && q.split('.').any(|seg| seg == "AnthropicBedrock"))
    {
        return Some(Some(hit(&BuiltinSink::finding(
            "anthropic.AnthropicBedrock",
            RULE_BLOCKING_HTTP,
        ))));
    }
    if q == "anthropic.AnthropicVertex"
        || q.starts_with("anthropic.AnthropicVertex.")
        || (q.starts_with("anthropic.lib.vertex.")
            && q.split('.').any(|seg| seg == "AnthropicVertex"))
    {
        return Some(Some(hit(&BuiltinSink::finding(
            "anthropic.AnthropicVertex",
            RULE_BLOCKING_HTTP,
        ))));
    }
    if q.starts_with("anthropic.Anthropic.")
        || (q.starts_with("anthropic.resources.") && !q.split('.').any(|seg| seg.starts_with('_')))
    {
        return Some(Some(hit(&BuiltinSink::finding(
            "anthropic.Anthropic",
            RULE_BLOCKING_HTTP,
        ))));
    }

    // Groq sync client & public resource methods
    if q == "groq.Groq"
        || q.starts_with("groq.Groq.")
        || (q.starts_with("groq.resources.") && !q.split('.').any(|seg| seg.starts_with('_')))
    {
        return Some(Some(hit(&BuiltinSink::finding(
            "groq.Groq",
            RULE_BLOCKING_HTTP,
        ))));
    }

    // Ollama sync Client & module-level RPC functions
    if q == "ollama.Client"
        || (q.starts_with("ollama.Client.")
            && !q
                .strip_prefix("ollama.Client.")
                .is_some_and(|m| m.starts_with('_')))
        || matches!(
            q,
            "ollama.chat"
                | "ollama.generate"
                | "ollama.embed"
                | "ollama.embeddings"
                | "ollama.list"
                | "ollama.show"
                | "ollama.pull"
                | "ollama.push"
                | "ollama.create"
                | "ollama.delete"
                | "ollama.copy"
                | "ollama.ps"
        )
    {
        return Some(Some(hit(&BuiltinSink::finding(
            "ollama.Client",
            RULE_BLOCKING_HTTP,
        ))));
    }

    // Qdrant sync QdrantClient (never `qdrant_client.models.*` / `http.models.*`)
    if q == "qdrant_client.QdrantClient"
        || (q.starts_with("qdrant_client.QdrantClient.")
            && !q
                .strip_prefix("qdrant_client.QdrantClient.")
                .is_some_and(|m| m.starts_with('_')))
    {
        return Some(Some(hit(&BuiltinSink::finding(
            "qdrant_client.QdrantClient",
            RULE_BLOCKING_HTTP,
        ))));
    }

    None
}

fn is_ai_client_copy_helper(q: &str) -> bool {
    let Some((receiver, method)) = q.rsplit_once('.') else {
        return false;
    };
    if !matches!(method, "with_options" | "copy") {
        return false;
    }
    matches!(
        receiver,
        "openai.OpenAI"
            | "openai.AzureOpenAI"
            | "openai.lib.azure.AzureOpenAI"
            | "anthropic.Anthropic"
            | "anthropic.AnthropicBedrock"
            | "anthropic.lib.bedrock.AnthropicBedrock"
            | "anthropic.AnthropicVertex"
            | "anthropic.lib.vertex.AnthropicVertex"
            | "groq.Groq"
    )
}

fn is_grpc_non_io_helper(q: &str) -> bool {
    let tail = q.rsplit_once('.').map(|(_, m)| m).unwrap_or(q);
    tail == "future"
        || q.contains("Credentials")
        || q.contains("Connectivity")
        || q.starts_with("grpc.StatusCode")
        || q.starts_with("grpc.Compression")
        || q.starts_with("grpc.RpcError")
        || tail.ends_with("_credentials")
}
