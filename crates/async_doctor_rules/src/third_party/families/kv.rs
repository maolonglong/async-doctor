//! Sync key-value datastore family matchers: Redis, RedisCluster, and Valkey.

use async_doctor_core::SinkHit;

use crate::third_party::hit;
use crate::{BuiltinSink, RULE_BLOCKING_DB};

use super::intern_qualname;

/// Returns `Some(Some(hit))` for sync Redis/Valkey IO, `Some(None)` for known
/// non-IO factories/buffers under `redis.*` / `valkey.*`, or `None` if unrelated.
pub(super) fn match_kv_family(q: &str) -> Option<Option<SinkHit>> {
    if !q.starts_with("redis.") && !q.starts_with("valkey.") {
        return None;
    }
    let is_valkey = q.starts_with("valkey.");

    // Sync Pipeline / ClusterPipeline buffering is not IO; only `.execute()` blocks.
    if is_redis_sync_pipeline_receiver(q) {
        if is_redis_pipeline_execute_method(q) {
            let sink_q = if is_valkey {
                "valkey.client.Pipeline.execute"
            } else if q.contains("ClusterPipeline") {
                "redis.cluster.ClusterPipeline.execute"
            } else {
                "redis.client.Pipeline.execute"
            };
            return Some(Some(hit(&BuiltinSink::finding(sink_q, RULE_BLOCKING_DB))));
        }
        return Some(None);
    }

    // Factories that return clients/pipelines/pubsub/lock handles are not IO.
    if q.ends_with(".pipeline") || q.ends_with(".pubsub") || is_kv_client_factory(q) {
        return Some(None);
    }

    // Distributed Lock.acquire blocks on network IO.
    if is_kv_sync_lock_acquire(q) {
        let sink_q = if is_valkey {
            "valkey.Lock.acquire"
        } else {
            "redis.Lock.acquire"
        };
        return Some(Some(hit(&BuiltinSink::finding(sink_q, RULE_BLOCKING_DB))));
    }

    // PubSub.listen() blocks only when the generator is consumed.
    if q.ends_with(".PubSub.listen") && !q.contains("asyncio") {
        let sink_q = if is_valkey {
            "valkey.PubSub.listen"
        } else {
            "redis.PubSub.listen"
        };
        return Some(Some(hit(&BuiltinSink::finding_consume_only(
            sink_q,
            RULE_BLOCKING_DB,
        ))));
    }

    // Other sync PubSub command methods (subscribe/get_message/…) block on use.
    if q.contains(".PubSub.") && !q.contains("asyncio") && !q.ends_with(".listen") {
        let sink_q = if is_valkey {
            "valkey.PubSub.get_message"
        } else {
            "redis.PubSub.get_message"
        };
        return Some(Some(hit(&BuiltinSink::finding(sink_q, RULE_BLOCKING_DB))));
    }

    let method = q
        .rsplit_once('.')
        .map(|(_, m)| m)
        .unwrap_or("execute_command");

    // RedisCluster sync client
    if (q.starts_with("redis.cluster.RedisCluster.") || q.starts_with("redis.RedisCluster."))
        && !q.contains("asyncio")
    {
        return Some(Some(SinkHit {
            rule: RULE_BLOCKING_DB,
            sink_qualname: redis_cluster_command_sink_qualname(method),
            fix_hint: "",
            candidate_only: false,
            candidate_reason: None,
            with_only: false,
            consume_only: false,
        }));
    }

    // Redis / StrictRedis sync client
    if (q.starts_with("redis.Redis.") || q.starts_with("redis.StrictRedis."))
        && !q.starts_with("redis.asyncio.")
    {
        return Some(Some(SinkHit {
            rule: RULE_BLOCKING_DB,
            sink_qualname: redis_command_sink_qualname(method),
            fix_hint: "",
            candidate_only: false,
            candidate_reason: None,
            with_only: false,
            consume_only: false,
        }));
    }

    // Valkey / StrictValkey / ValkeyCluster sync clients
    if (q.starts_with("valkey.Valkey.")
        || q.starts_with("valkey.StrictValkey.")
        || q.starts_with("valkey.ValkeyCluster.")
        || q.starts_with("valkey.cluster.ValkeyCluster."))
        && !q.contains("asyncio")
    {
        return Some(Some(SinkHit {
            rule: RULE_BLOCKING_DB,
            sink_qualname: valkey_command_sink_qualname(method),
            fix_hint: "",
            candidate_only: false,
            candidate_reason: None,
            with_only: false,
            consume_only: false,
        }));
    }

    None
}

/// True when the External qualname is a method on a *sync* Redis/Valkey Pipeline receiver.
pub(in crate::third_party) fn is_redis_sync_pipeline_receiver(q: &str) -> bool {
    if q.contains("asyncio") || (!q.starts_with("redis.") && !q.starts_with("valkey.")) {
        return false;
    }
    q.split('.')
        .any(|seg| matches!(seg, "Pipeline" | "ClusterPipeline"))
}

fn is_redis_pipeline_execute_method(q: &str) -> bool {
    q.ends_with(".execute")
        || q.ends_with(".execute_command")
        || q.ends_with(".immediate_execute_command")
}

/// `client.pipeline()` / `pipe.set()` / `pipe.get()` only buffer; sync `pipe.execute()` blocks.
pub(in crate::third_party) fn is_redis_pipeline_buffering(q: &str) -> bool {
    is_redis_sync_pipeline_receiver(q) && !is_redis_pipeline_execute_method(q)
}

fn is_kv_client_factory(q: &str) -> bool {
    let method = q.rsplit_once('.').map(|(_, m)| m).unwrap_or(q);
    matches!(
        method,
        "from_url"
            | "from_pool"
            | "from_startup_nodes"
            | "__init__"
            | "__new__"
            | "Redis"
            | "StrictRedis"
            | "RedisCluster"
            | "Valkey"
            | "StrictValkey"
            | "ValkeyCluster"
            | "lock"
    )
}

fn is_kv_sync_lock_acquire(q: &str) -> bool {
    if q.contains("asyncio") || (!q.starts_with("redis.") && !q.starts_with("valkey.")) {
        return false;
    }
    q.split('.').any(|seg| seg == "Lock") && (q.ends_with(".acquire") || q.ends_with(".__enter__"))
}

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

fn redis_cluster_command_sink_qualname(method: &str) -> &'static str {
    match method {
        "get" => "redis.cluster.RedisCluster.get",
        "set" => "redis.cluster.RedisCluster.set",
        "execute_command" => "redis.cluster.RedisCluster.execute_command",
        other => intern_qualname(&format!("redis.cluster.RedisCluster.{other}")),
    }
}

fn valkey_command_sink_qualname(method: &str) -> &'static str {
    match method {
        "get" => "valkey.Valkey.get",
        "set" => "valkey.Valkey.set",
        "execute_command" => "valkey.Valkey.execute_command",
        other => intern_qualname(&format!("valkey.Valkey.{other}")),
    }
}
