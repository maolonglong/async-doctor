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
fn sqlalchemy_non_io_session_methods_never_fire() {
    let t = BuiltinSinkTable::new();
    for q in [
        "sqlalchemy.orm.Session.add",
        "sqlalchemy.orm.Session.add_all",
        "sqlalchemy.orm.Session.expunge",
        "sqlalchemy.orm.Session.expunge_all",
        "sqlalchemy.orm.Session.expire",
        "sqlalchemy.orm.Session._is_clean",
        "sqlalchemy.orm.Session._expunge_states",
    ] {
        assert!(t.lookup(q).is_none(), "{q} must not be a sink");
    }
    let commit = t.lookup("sqlalchemy.orm.Session.commit").unwrap();
    assert_eq!(commit.sink_qualname, "sqlalchemy.orm.Session.commit");
    let get = t.lookup("sqlalchemy.orm.Session.get").unwrap();
    assert_eq!(get.sink_qualname, "sqlalchemy.orm.Session.get");
    assert!(
        t.lookup("sqlalchemy.orm.Session.bulk_save_objects")
            .is_some()
    );
    assert!(
        t.lookup("sqlalchemy.orm.Session.bulk_insert_mappings")
            .is_some()
    );
    assert!(
        t.lookup("sqlalchemy.orm.Session.bulk_update_mappings")
            .is_some()
    );
    let qiter = t.lookup("sqlalchemy.orm.Query.__iter__").unwrap();
    assert_eq!(qiter.sink_qualname, "sqlalchemy.orm.Query.__iter__");
    assert!(t.lookup("redis.Redis.lock").is_none());
}

#[test]
fn redis_factory_and_command_labels() {
    let t = BuiltinSinkTable::new();
    assert!(t.lookup("redis.Redis.from_url").is_none());
    assert!(t.lookup("redis.Redis.from_pool").is_none());
    let get = t.lookup("redis.Redis.get").unwrap();
    assert_eq!(get.sink_qualname, "redis.Redis.get");
    let hget = t.lookup("redis.Redis.hget").unwrap();
    assert_eq!(hget.sink_qualname, "redis.Redis.hget");
}

#[test]
fn urllib_request_urlopen_not_urllib3() {
    let t = BuiltinSinkTable::new();
    let hit = t.lookup("urllib.request.urlopen").unwrap();
    assert_eq!(hit.sink_qualname, "urllib.request.urlopen");
    assert_eq!(hit.rule, RULE_BLOCKING_HTTP);
    let u3 = t.lookup("urllib3.HTTPConnectionPool.urlopen").unwrap();
    assert_eq!(u3.rule, RULE_BLOCKING_HTTP);
    // Family catch-all still labels as urllib3.request for non-exact rows.
    let family = t
        .lookup("urllib3.connectionpool.HTTPConnectionPool.urlopen")
        .unwrap();
    assert!(
        family.sink_qualname.contains("urllib3"),
        "{}",
        family.sink_qualname
    );
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

#[test]
fn azure_openai_is_blocking_sink() {
    let t = BuiltinSinkTable::new();
    let hit = t.lookup("openai.lib.azure.AzureOpenAI.complete");
    assert!(hit.is_some(), "azure method must be a sink");
    let twin = t
        .async_twin("openai.lib.azure.AzureOpenAI.complete")
        .unwrap();
    assert_eq!(twin.symbol, "openai.AsyncAzureOpenAI");
}

#[test]
fn boto_dynamic_client_methods_match_aioboto_advice() {
    let t = BuiltinSinkTable::new();
    let hit = t
        .lookup("botocore.client.BaseClient.get_object")
        .expect("get_object");
    assert_eq!(hit.rule, RULE_BLOCKING_HTTP);
    assert!(hit.fix_hint.contains("aioboto3"));
    assert!(t.lookup("botocore.client.BaseClient.meta").is_none());
    assert!(t.lookup("botocore.client.BaseClient.get_paginator").is_none());
    assert!(t.lookup("botocore.client.BaseClient.get_waiter").is_none());
    assert!(t.lookup("botocore.client.BaseClient.can_paginate").is_none());
    assert!(t
        .lookup("botocore.client.BaseClient.generate_presigned_url")
        .is_none());
    assert!(t.lookup("boto3.client").is_some());
}

#[test]
fn stdlib_p1_sinks_resolve() {
    let t = BuiltinSinkTable::new();
    for q in [
        "smtplib.SMTP.sendmail",
        "ftplib.FTP.login",
        "zipfile.ZipFile.extractall",
        "tarfile.open",
        "glob.glob",
        "pathlib.Path.iterdir",
        "builtins.input",
        "concurrent.futures._base.Future.result",
        "concurrent.futures.ThreadPoolExecutor.map",
        "concurrent.futures.wait",
    ] {
        assert!(t.lookup(q).is_some(), "missing sink for {q}");
    }
    // asyncio Future must not match concurrent.futures normalization.
    assert!(t.lookup("asyncio.futures.Future.result").is_none());
    assert!(t.lookup("asyncio.Future.result").is_none());
}
