//! Third-party sink table unit tests.

use async_doctor_core::SinkTable;

use super::BuiltinSinkTable;
use crate::RULE_BLOCKING_HTTP;

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
    assert!(t.lookup("openai.OpenAI.chat.completions.create").is_some());
    // Helpers under openai.lib that merely contain "completions" must not match.
    assert!(
        t.lookup("openai.lib._parsing._completions._materialize_input_tools")
            .is_none()
    );
    assert!(
        t.lookup("openai.resources.chat.completions.AsyncCompletions.create")
            .is_none()
    );
}

#[test]
fn stdlib_still_resolves() {
    let t = BuiltinSinkTable::new();
    assert!(t.lookup("time.sleep").is_some());
}

#[test]
fn module_path_async_client_is_counterpart() {
    assert!(crate::is_async_counterpart("httpx.AsyncClient.get"));
    assert!(crate::is_async_counterpart("httpx._client.AsyncClient.get"));
    assert!(crate::is_async_counterpart(
        "elasticsearch.AsyncElasticsearch.search"
    ));
    assert!(crate::is_async_counterpart(
        "elasticsearch._async.client.AsyncElasticsearch.search"
    ));
    // Sync must stay false.
    assert!(!crate::is_async_counterpart("httpx.Client.get"));
    assert!(!crate::is_async_counterpart("httpx._client.Client.get"));
}
