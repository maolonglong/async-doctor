//! Regression tests for mainstream third-party sink/twin gaps and DB guidance.

use async_doctor_core::SinkTable;

use crate::{
    BuiltinSinkTable, RULE_BLOCKING_DB, RULE_BLOCKING_HTTP, is_async_counterpart, rule_doc,
};

#[test]
fn blocking_db_docs_and_hints_never_recommend_shared_session_to_thread() {
    let doc = rule_doc("blocking-db").expect("blocking-db doc");
    let full = doc.explain_text();
    for forbidden in [
        "to_thread(session.execute",
        "to_thread(session.exec",
        "to_thread(conn.execute",
    ] {
        assert!(
            !full.contains(forbidden),
            "blocking-db explain_text must not recommend {forbidden}: {full}"
        );
    }
    assert!(
        full.contains("unit-of-work") || full.contains("opens") || full.contains("worker thread"),
        "blocking-db explain_text must explain worker-owned Session/Connection: {full}"
    );

    let t = BuiltinSinkTable::new();
    for q in [
        "sqlalchemy.orm.Session.execute",
        "sqlalchemy.orm.Session.commit",
        "sqlmodel.orm.session.Session.exec",
        "sqlmodel.Session.exec",
        "sqlite3.Connection.execute",
    ] {
        let hit = t
            .lookup(q)
            .unwrap_or_else(|| panic!("expected sink for {q}"));
        assert_eq!(hit.rule, RULE_BLOCKING_DB);
        assert!(
            !hit.fix_hint.contains("to_thread(session.execute")
                && !hit.fix_hint.contains("to_thread(conn.execute"),
            "unsafe fix_hint on {q}: {}",
            hit.fix_hint
        );
    }
}

#[test]
fn httpx_module_verbs_and_stream_match_sinks_and_twins() {
    let t = BuiltinSinkTable::new();
    for q in [
        "httpx.get",
        "httpx.post",
        "httpx.put",
        "httpx.patch",
        "httpx.delete",
        "httpx.head",
        "httpx.options",
        "httpx.request",
        "httpx.stream",
        "httpx._api.put",
        "httpx._api.stream",
    ] {
        let hit = t
            .lookup(q)
            .unwrap_or_else(|| panic!("expected sink for {q}"));
        assert_eq!(hit.rule, RULE_BLOCKING_HTTP);
        let twin = t
            .async_twin(hit.sink_qualname)
            .unwrap_or_else(|| panic!("expected async twin for {q} -> {}", hit.sink_qualname));
        assert_eq!(twin.symbol, "httpx.AsyncClient");
    }
    for async_q in [
        "httpx.AsyncClient.put",
        "httpx.AsyncClient.patch",
        "httpx.AsyncClient.delete",
        "httpx.AsyncClient.head",
        "httpx.AsyncClient.options",
        "httpx.AsyncClient.stream",
    ] {
        assert!(t.lookup(async_q).is_none(), "{async_q} must never fire");
    }
}

#[test]
fn boto3_and_botocore_high_frequency_ops_waiters_and_streaming_body() {
    let t = BuiltinSinkTable::new();
    for op in [
        "head_object",
        "head_bucket",
        "copy_object",
        "upload_part",
        "complete_multipart_upload",
        "abort_multipart_upload",
        "converse",
        "converse_stream",
        "invoke_model",
        "invoke_model_with_response_stream",
        "batch_get_item",
        "batch_write_item",
        "transact_get_items",
        "transact_write_items",
        "execute_statement",
        "assume_role",
        "assume_role_with_web_identity",
        "assume_role_with_saml",
        "decrypt",
        "encrypt",
        "generate_data_key",
    ] {
        assert!(
            crate::aws_op_method_name(op),
            "aws_op_method_name must recognize {op}"
        );
        let q = format!("botocore.client.BaseClient.{op}");
        let hit = t
            .lookup(&q)
            .unwrap_or_else(|| panic!("expected sink for {q}"));
        assert_eq!(hit.rule, RULE_BLOCKING_HTTP);
        assert!(
            t.async_twin(&q).is_none(),
            "boto must stay excluded from twin advice"
        );
    }

    for q in [
        "botocore.waiter.Waiter.wait",
        "botocore.client.BaseClient.wait",
        "botocore.response.StreamingBody.read",
        "botocore.response.StreamingBody.iter_lines",
        "botocore.response.StreamingBody.iter_chunks",
    ] {
        let hit = t
            .lookup(q)
            .unwrap_or_else(|| panic!("expected sink for {q}"));
        assert_eq!(hit.rule, RULE_BLOCKING_HTTP);
    }

    // Async aioboto3 / aiobotocore counterparts and local metadata helpers must never fire.
    for clean in [
        "aiobotocore.response.StreamingBody.read",
        "aiobotocore.response.StreamingBody.iter_lines",
        "aiobotocore.waiter.AIOWaiter.wait",
        "aioboto3.Session.client",
        "botocore.client.BaseClient.get_waiter",
        "botocore.client.BaseClient.get_paginator",
    ] {
        assert!(t.lookup(clean).is_none(), "{clean} must not be a sink");
    }
}

#[test]
fn sqlmodel_redis_cluster_valkey_and_pymongo_collection_ops() {
    let t = BuiltinSinkTable::new();

    // SQLModel sync vs async + non-IO Session helpers
    for q in [
        "sqlmodel.orm.session.Session.exec",
        "sqlmodel.Session.exec",
        "sqlmodel.orm.session.Session.commit",
        "sqlmodel.Session.get",
    ] {
        let hit = t
            .lookup(q)
            .unwrap_or_else(|| panic!("expected sink for {q}"));
        assert_eq!(hit.rule, RULE_BLOCKING_DB);
        let twin = t
            .async_twin(q)
            .or_else(|| t.async_twin(hit.sink_qualname))
            .unwrap();
        assert_eq!(twin.symbol, "sqlmodel.ext.asyncio.session.AsyncSession");
        assert_eq!(twin.needs, &["greenlet"]);
    }
    for clean in [
        "sqlmodel.ext.asyncio.session.AsyncSession.exec",
        "sqlmodel.ext.asyncio.session.AsyncSession.commit",
        "sqlmodel.Session.add",
        "sqlmodel.orm.session.Session.add",
        "sqlmodel.orm.session.Session.expunge",
    ] {
        assert!(t.lookup(clean).is_none(), "{clean} must not be a sink");
    }
    assert!(is_async_counterpart(
        "sqlmodel.ext.asyncio.session.AsyncSession.exec"
    ));

    // RedisCluster sync vs asyncio + ClusterPipeline buffering
    for q in [
        "redis.cluster.RedisCluster.get",
        "redis.cluster.RedisCluster.set",
        "redis.cluster.RedisCluster.execute_command",
        "redis.RedisCluster.get",
        "redis.cluster.ClusterPipeline.execute",
    ] {
        let hit = t
            .lookup(q)
            .unwrap_or_else(|| panic!("expected sink for {q}"));
        assert_eq!(hit.rule, RULE_BLOCKING_DB);
        let twin = t
            .async_twin(q)
            .or_else(|| t.async_twin(hit.sink_qualname))
            .unwrap();
        assert_eq!(twin.symbol, "redis.asyncio");
    }
    for clean in [
        "redis.asyncio.cluster.RedisCluster.get",
        "redis.asyncio.RedisCluster.get",
        "redis.asyncio.cluster.ClusterPipeline.execute",
        "redis.cluster.RedisCluster.from_url",
        "redis.cluster.RedisCluster.pipeline",
        "redis.cluster.ClusterPipeline.set",
    ] {
        assert!(t.lookup(clean).is_none(), "{clean} must not be a sink");
    }

    // Valkey sync vs valkey.asyncio
    for q in [
        "valkey.Valkey.get",
        "valkey.Valkey.set",
        "valkey.client.Valkey.ping",
        "valkey.cluster.ValkeyCluster.get",
        "valkey.client.Pipeline.execute",
    ] {
        let hit = t
            .lookup(q)
            .unwrap_or_else(|| panic!("expected sink for {q}"));
        assert_eq!(hit.rule, RULE_BLOCKING_DB);
        let twin = t
            .async_twin(q)
            .or_else(|| t.async_twin(hit.sink_qualname))
            .unwrap();
        assert_eq!(twin.symbol, "valkey.asyncio.Valkey");
    }
    for clean in [
        "valkey.asyncio.Valkey.get",
        "valkey.asyncio.client.Valkey.get",
        "valkey.asyncio.cluster.ValkeyCluster.get",
        "valkey.asyncio.client.Pipeline.execute",
        "valkey.Valkey.from_url",
        "valkey.Valkey.pipeline",
        "valkey.client.Pipeline.set",
    ] {
        assert!(t.lookup(clean).is_none(), "{clean} must not be a sink");
    }
    assert!(is_async_counterpart("valkey.asyncio.Valkey.get"));
    assert!(is_async_counterpart("valkey.asyncio.client.Valkey.get"));

    // PyMongo Collection methods + PyMongo async exclusions
    for q in [
        "pymongo.collection.Collection.count_documents",
        "pymongo.collection.Collection.estimated_document_count",
        "pymongo.collection.Collection.distinct",
        "pymongo.collection.Collection.replace_one",
        "pymongo.collection.Collection.bulk_write",
    ] {
        let hit = t
            .lookup(q)
            .unwrap_or_else(|| panic!("expected sink for {q}"));
        assert_eq!(hit.rule, RULE_BLOCKING_DB);
        assert!(
            t.async_twin(q).is_none(),
            "pymongo remains excluded from twin advice"
        );
    }
    for clean in [
        "pymongo.asynchronous.collection.AsyncCollection.count_documents",
        "pymongo.AsyncMongoClient",
        "pymongo.collection.Collection.with_options",
    ] {
        assert!(t.lookup(clean).is_none(), "{clean} must not be a sink");
    }
}

#[test]
fn anthropic_cloud_confluent_kafka_groq_ollama_and_qdrant_sinks_and_twins() {
    let t = BuiltinSinkTable::new();

    // AnthropicBedrock & AnthropicVertex
    for (sync_q, expected_twin) in [
        (
            "anthropic.AnthropicBedrock",
            "anthropic.AsyncAnthropicBedrock",
        ),
        (
            "anthropic.AnthropicBedrock.messages.create",
            "anthropic.AsyncAnthropicBedrock",
        ),
        (
            "anthropic.lib.bedrock.AnthropicBedrock.messages.create",
            "anthropic.AsyncAnthropicBedrock",
        ),
        (
            "anthropic.AnthropicVertex",
            "anthropic.AsyncAnthropicVertex",
        ),
        (
            "anthropic.AnthropicVertex.messages.create",
            "anthropic.AsyncAnthropicVertex",
        ),
        (
            "anthropic.lib.vertex.AnthropicVertex.messages.create",
            "anthropic.AsyncAnthropicVertex",
        ),
    ] {
        let hit = t
            .lookup(sync_q)
            .unwrap_or_else(|| panic!("expected sink for {sync_q}"));
        assert_eq!(hit.rule, RULE_BLOCKING_HTTP);
        let twin = t
            .async_twin(sync_q)
            .or_else(|| t.async_twin(hit.sink_qualname))
            .unwrap();
        assert_eq!(twin.symbol, expected_twin);
    }
    for clean in [
        "anthropic.AsyncAnthropicBedrock",
        "anthropic.AsyncAnthropicBedrock.messages.create",
        "anthropic.lib.bedrock.AsyncAnthropicBedrock.messages.create",
        "anthropic.AsyncAnthropicVertex",
        "anthropic.AsyncAnthropicVertex.messages.create",
        "anthropic.lib.vertex.AsyncAnthropicVertex.messages.create",
        "anthropic.AnthropicBedrock.with_options",
        "anthropic.AnthropicBedrock.copy",
        "anthropic.AnthropicVertex.with_options",
        "anthropic.AnthropicVertex.copy",
    ] {
        assert!(t.lookup(clean).is_none(), "{clean} must not be a sink");
    }
    assert!(is_async_counterpart("anthropic.AsyncAnthropicVertex"));
    assert!(is_async_counterpart("anthropic.AsyncAnthropicBedrock"));

    // confluent_kafka
    for q in [
        "confluent_kafka.Producer.flush",
        "confluent_kafka.Consumer.poll",
        "confluent_kafka.Consumer.consume",
        "confluent_kafka.cimpl.Producer.flush",
        "confluent_kafka.cimpl.Consumer.poll",
        "confluent_kafka.cimpl.Consumer.consume",
    ] {
        let hit = t
            .lookup(q)
            .unwrap_or_else(|| panic!("expected sink for {q}"));
        assert_eq!(hit.rule, RULE_BLOCKING_HTTP);
        assert!(t.async_twin(q).is_none());
    }
    for clean in [
        "confluent_kafka.Producer.produce",
        "confluent_kafka.Consumer.subscribe",
        "confluent_kafka.experimental.aio.AIOProducer.produce",
        "confluent_kafka.experimental.aio.AIOConsumer.poll",
    ] {
        assert!(t.lookup(clean).is_none(), "{clean} must not be a sink");
    }

    // Groq
    for q in [
        "groq.Groq",
        "groq.Groq.chat.completions.create",
        "groq._client.Groq.chat.completions.create",
        "groq.resources.chat.completions.Completions.create",
    ] {
        let hit = t
            .lookup(q)
            .unwrap_or_else(|| panic!("expected sink for {q}"));
        assert_eq!(hit.rule, RULE_BLOCKING_HTTP);
        let twin = t
            .async_twin(q)
            .or_else(|| t.async_twin(hit.sink_qualname))
            .unwrap();
        assert_eq!(twin.symbol, "groq.AsyncGroq");
    }
    for clean in [
        "groq.AsyncGroq",
        "groq.AsyncGroq.chat.completions.create",
        "groq._client.AsyncGroq.chat.completions.create",
        "groq.resources.chat.completions.AsyncCompletions.create",
        "groq.Groq.with_options",
        "groq.Groq.copy",
    ] {
        assert!(t.lookup(clean).is_none(), "{clean} must not be a sink");
    }
    assert!(is_async_counterpart(
        "groq.AsyncGroq.chat.completions.create"
    ));

    // Ollama
    for q in [
        "ollama.Client",
        "ollama.Client.chat",
        "ollama.Client.generate",
        "ollama._client.Client.embed",
        "ollama.chat",
        "ollama.generate",
        "ollama.embed",
    ] {
        let hit = t
            .lookup(q)
            .unwrap_or_else(|| panic!("expected sink for {q}"));
        assert_eq!(hit.rule, RULE_BLOCKING_HTTP);
        let twin = t
            .async_twin(q)
            .or_else(|| t.async_twin(hit.sink_qualname))
            .unwrap();
        assert_eq!(twin.symbol, "ollama.AsyncClient");
    }
    for clean in [
        "ollama.AsyncClient",
        "ollama.AsyncClient.chat",
        "ollama._client.AsyncClient.chat",
        "ollama.Message",
    ] {
        assert!(t.lookup(clean).is_none(), "{clean} must not be a sink");
    }
    assert!(is_async_counterpart("ollama.AsyncClient.chat"));

    // Qdrant
    for q in [
        "qdrant_client.QdrantClient",
        "qdrant_client.QdrantClient.search",
        "qdrant_client.QdrantClient.query_points",
        "qdrant_client.QdrantClient.upsert",
        "qdrant_client.qdrant_client.QdrantClient.query_points",
    ] {
        let hit = t
            .lookup(q)
            .unwrap_or_else(|| panic!("expected sink for {q}"));
        assert_eq!(hit.rule, RULE_BLOCKING_HTTP);
        let twin = t
            .async_twin(q)
            .or_else(|| t.async_twin(hit.sink_qualname))
            .unwrap();
        assert_eq!(twin.symbol, "qdrant_client.AsyncQdrantClient");
    }
    for clean in [
        "qdrant_client.AsyncQdrantClient",
        "qdrant_client.AsyncQdrantClient.search",
        "qdrant_client.async_qdrant_client.AsyncQdrantClient.query_points",
        "qdrant_client.models.PointStruct",
        "qdrant_client.http.models.Filter",
    ] {
        assert!(t.lookup(clean).is_none(), "{clean} must not be a sink");
    }
    assert!(is_async_counterpart(
        "qdrant_client.AsyncQdrantClient.search"
    ));
}
