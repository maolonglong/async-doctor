//! Sync SQL and document database family matchers: SQLAlchemy, SQLModel, Postgres, MySQL, MongoDB.

use async_doctor_core::SinkHit;

use crate::third_party::hit;
use crate::{BuiltinSink, RULE_BLOCKING_DB};

use super::intern_qualname;

pub(super) fn match_sql_or_doc_family(q: &str) -> Option<SinkHit> {
    if let Some(hit) = match_sqlalchemy_or_sqlmodel_family(q) {
        return Some(hit);
    }
    // psycopg / psycopg2
    if (q.starts_with("psycopg2.")
        || (q.starts_with("psycopg.") && !q.starts_with("psycopg.Async")))
        && (q.contains("connect")
            || q.contains("execute")
            || q.contains("fetch")
            || q.contains("cursor")
            || q.contains("commit"))
    {
        return Some(hit(&BuiltinSink::finding(
            "psycopg.connect",
            RULE_BLOCKING_DB,
        )));
    }
    // pymysql — exception constructors, constants, converters, and pure string/type
    // helpers are not IO; only the driver surface stays a sink.
    if q.starts_with("pymysql.")
        && !q.starts_with("pymysql.err.")
        && !q.starts_with("pymysql.constants.")
        && !q.starts_with("pymysql.converters.")
        && !is_pymysql_non_io_helper(q)
    {
        return Some(hit(&BuiltinSink::finding(
            "pymysql.connect",
            RULE_BLOCKING_DB,
        )));
    }
    // pymongo sync — exclude pure handle getters (`get_database`, `get_collection`,
    // `with_options`) that construct client-side wrappers without network IO.
    if q.starts_with("pymongo.")
        && !q.starts_with("pymongo.asynchronous.")
        && !q.contains("Async")
        && !q.starts_with("pymongo.errors.")
        && !q.starts_with("pymongo.uri_parser.")
        && !q.starts_with("pymongo.client_options.")
        && !is_pymongo_non_io_helper(q)
        && (q.contains("find")
            || q.contains("insert")
            || q.contains("update")
            || q.contains("delete")
            || q.contains("aggregate")
            || q.ends_with(".MongoClient")
            || is_pymongo_collection_blocking_op(q)
            || is_pymongo_client_blocking_op(q))
    {
        return Some(hit(&BuiltinSink::finding(
            "pymongo.collection.Collection.find",
            RULE_BLOCKING_DB,
        )));
    }
    None
}

fn match_sqlalchemy_or_sqlmodel_family(q: &str) -> Option<SinkHit> {
    // SQLModel sync Session (inherits SQLAlchemy Session IO methods + `.exec()`).
    if let Some(method) = sqlmodel_session_io_method(q) {
        let sink_qualname = match method {
            "exec" => "sqlmodel.Session.exec",
            other => intern_qualname(&format!("sqlmodel.Session.{other}")),
        };
        return Some(SinkHit {
            rule: RULE_BLOCKING_DB,
            sink_qualname,
            fix_hint: "",
            candidate_only: false,
            candidate_reason: None,
            with_only: false,
            consume_only: false,
        });
    }

    // SQLAlchemy sync Session: only methods that emit SQL / talk to the DB.
    if let Some(method) = sqlalchemy_session_io_method(q) {
        return Some(SinkHit {
            rule: RULE_BLOCKING_DB,
            sink_qualname: sqlalchemy_session_sink_qualname(method),
            fix_hint: "",
            candidate_only: false,
            candidate_reason: None,
            with_only: false,
            consume_only: false,
        });
    }

    // Legacy Query: iteration / fetch emits SQL (`for u in session.query(User)`).
    if let Some(method) = sqlalchemy_query_io(q) {
        let sink_qualname = if method == "__iter__" {
            "sqlalchemy.orm.Query.__iter__"
        } else {
            intern_qualname(&format!("sqlalchemy.orm.Query.{method}"))
        };
        return Some(SinkHit {
            rule: RULE_BLOCKING_DB,
            sink_qualname,
            fix_hint: "",
            candidate_only: false,
            candidate_reason: None,
            with_only: false,
            consume_only: false,
        });
    }

    if q.starts_with("sqlalchemy.engine.Engine.")
        && !q.contains("begin")
        && (q.ends_with(".connect") || q.ends_with(".execute") || q.ends_with(".raw_connection"))
    {
        return Some(hit(&BuiltinSink::finding(
            "sqlalchemy.engine.Engine.connect",
            RULE_BLOCKING_DB,
        )));
    }

    if q == "sqlalchemy.engine.Connection.execute"
        || q == "sqlalchemy.engine.Connection.exec_driver_sql"
        || q == "sqlalchemy.engine.Connection.scalar"
    {
        return Some(hit(&BuiltinSink::finding(
            "sqlalchemy.engine.Connection.execute",
            RULE_BLOCKING_DB,
        )));
    }

    if q == "sqlalchemy.engine.Engine.begin"
        || q == "sqlalchemy.engine.Engine.begin.__enter__"
        || q.ends_with("Engine.begin")
    {
        return Some(hit(&BuiltinSink::finding_with_only(
            "sqlalchemy.engine.Engine.begin",
            RULE_BLOCKING_DB,
        )));
    }

    None
}

fn session_io_method_name(method: &str) -> Option<&'static str> {
    if method.starts_with('_') {
        return None;
    }
    match method {
        "execute" => Some("execute"),
        "scalar" => Some("scalar"),
        "scalars" => Some("scalars"),
        "get" => Some("get"),
        "get_one" => Some("get_one"),
        "get_one_or_none" => Some("get_one_or_none"),
        "commit" => Some("commit"),
        "flush" => Some("flush"),
        "rollback" => Some("rollback"),
        "refresh" => Some("refresh"),
        "connection" => Some("connection"),
        "merge" => Some("merge"),
        "close" => Some("close"),
        "scalar_one" => Some("scalar_one"),
        "scalar_one_or_none" => Some("scalar_one_or_none"),
        "bulk_save_objects" => Some("bulk_save_objects"),
        "bulk_insert_mappings" => Some("bulk_insert_mappings"),
        "bulk_update_mappings" => Some("bulk_update_mappings"),
        _ => None,
    }
}

fn sqlalchemy_session_io_method(q: &str) -> Option<&'static str> {
    let method = q
        .strip_prefix("sqlalchemy.orm.Session.")
        .or_else(|| q.strip_prefix("sqlalchemy.orm.session.Session."))?;
    session_io_method_name(method)
}

fn sqlmodel_session_io_method(q: &str) -> Option<&'static str> {
    let method = q
        .strip_prefix("sqlmodel.Session.")
        .or_else(|| q.strip_prefix("sqlmodel.orm.session.Session."))?;
    if method == "exec" {
        return Some("exec");
    }
    session_io_method_name(method)
}

fn sqlalchemy_session_sink_qualname(method: &str) -> &'static str {
    match method {
        "execute" => "sqlalchemy.orm.Session.execute",
        "scalar" => "sqlalchemy.orm.Session.scalar",
        "scalars" => "sqlalchemy.orm.Session.scalars",
        "get" => "sqlalchemy.orm.Session.get",
        "get_one" => "sqlalchemy.orm.Session.get_one",
        "get_one_or_none" => "sqlalchemy.orm.Session.get_one_or_none",
        "commit" => "sqlalchemy.orm.Session.commit",
        "flush" => "sqlalchemy.orm.Session.flush",
        "rollback" => "sqlalchemy.orm.Session.rollback",
        "refresh" => "sqlalchemy.orm.Session.refresh",
        "connection" => "sqlalchemy.orm.Session.connection",
        "merge" => "sqlalchemy.orm.Session.merge",
        "close" => "sqlalchemy.orm.Session.close",
        "scalar_one" => "sqlalchemy.orm.Session.scalar_one",
        "scalar_one_or_none" => "sqlalchemy.orm.Session.scalar_one_or_none",
        "bulk_save_objects" => "sqlalchemy.orm.Session.bulk_save_objects",
        "bulk_insert_mappings" => "sqlalchemy.orm.Session.bulk_insert_mappings",
        "bulk_update_mappings" => "sqlalchemy.orm.Session.bulk_update_mappings",
        _ => "sqlalchemy.orm.Session.execute",
    }
}

fn sqlalchemy_query_io(q: &str) -> Option<&'static str> {
    let method = q
        .strip_prefix("sqlalchemy.orm.Query.")
        .or_else(|| q.strip_prefix("sqlalchemy.orm.query.Query."))?;
    match method {
        "__iter__" => Some("__iter__"),
        "all" => Some("all"),
        "first" => Some("first"),
        "one" => Some("one"),
        "one_or_none" => Some("one_or_none"),
        "scalar" => Some("scalar"),
        "get" => Some("get"),
        "count" => Some("count"),
        "delete" => Some("delete"),
        "update" => Some("update"),
        _ => None,
    }
}

fn is_pymysql_non_io_helper(q: &str) -> bool {
    let tail = q.rsplit_once('.').map(|(_, m)| m).unwrap_or(q);
    if matches!(
        q,
        "pymysql.Binary"
            | "pymysql.Date"
            | "pymysql.Time"
            | "pymysql.Timestamp"
            | "pymysql.DateFromTicks"
            | "pymysql.TimeFromTicks"
            | "pymysql.TimestampFromTicks"
            | "pymysql.escape_string"
            | "pymysql.escape_dict"
            | "pymysql.escape_sequence"
            | "pymysql.install_as_MySQLdb"
            | "pymysql.get_client_info"
            | "pymysql.version_info"
    ) {
        return true;
    }
    q.contains("Connection.")
        && matches!(
            tail,
            "escape"
                | "escape_string"
                | "literal"
                | "get_proto_info"
                | "get_host_info"
                | "get_server_info"
                | "open"
        )
}

fn is_pymongo_non_io_helper(q: &str) -> bool {
    let tail = q.rsplit_once('.').map(|(_, m)| m).unwrap_or(q);
    matches!(
        tail,
        "get_database"
            | "get_default_database"
            | "get_collection"
            | "with_options"
            | "codec_options"
            | "read_preference"
            | "write_concern"
            | "read_concern"
    )
}

fn is_pymongo_collection_blocking_op(q: &str) -> bool {
    if !q.contains("Collection.") {
        return false;
    }
    let tail = q.rsplit_once('.').map(|(_, m)| m).unwrap_or("");
    matches!(
        tail,
        "count_documents"
            | "estimated_document_count"
            | "distinct"
            | "replace_one"
            | "bulk_write"
            | "create_index"
            | "create_indexes"
            | "drop_index"
            | "drop_indexes"
            | "drop"
            | "rename"
            | "watch"
    )
}

fn is_pymongo_client_blocking_op(q: &str) -> bool {
    if !q.contains("MongoClient.") {
        return false;
    }
    let tail = q.rsplit_once('.').map(|(_, m)| m).unwrap_or("");
    matches!(
        tail,
        "__init__"
            | "__new__"
            | "close"
            | "server_info"
            | "list_database_names"
            | "list_databases"
            | "drop_database"
            | "start_session"
            | "watch"
            | "bulk_write"
    )
}
