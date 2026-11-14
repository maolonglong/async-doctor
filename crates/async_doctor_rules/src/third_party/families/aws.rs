//! Sync `boto3` / `botocore` package, factory, waiter, stream, and operation matchers.

use std::borrow::Cow;

use async_doctor_core::SinkHit;

use crate::third_party::hit;
use crate::{BuiltinSink, RULE_BLOCKING_HTTP};

pub(super) fn match_aws_family(q: &str) -> Option<SinkHit> {
    // boto3 / botocore — factories + dynamic client/resource ops + waiters + response bodies.
    // Avoid matching arbitrary `*.client.*` helpers (e.g. BaseClient.meta).
    // `boto3-stubs.*` arrives pre-normalized via [`crate::third_party::normalize_third_party`].
    if !is_sync_boto_package_qual(q) {
        return None;
    }
    if is_sync_boto_factory_qualname(q) {
        let key = if q.contains("resource") {
            "boto3.resource"
        } else {
            "boto3.client"
        };
        return Some(hit(&BuiltinSink::finding(
            key,
            RULE_BLOCKING_HTTP,
            "Use `aioboto3` / `aiobotocore` instead of sync `boto3`.",
        )));
    }
    if q.starts_with("botocore.waiter.") && q.ends_with(".wait") {
        return Some(hit(&BuiltinSink::finding(
            "botocore.waiter.Waiter.wait",
            RULE_BLOCKING_HTTP,
            "Use `aioboto3` / `aiobotocore` waiters (`await waiter.wait(...)`), or offload.",
        )));
    }
    if matches!(
        q,
        "botocore.response.StreamingBody.read"
            | "botocore.response.StreamingBody.iter_lines"
            | "botocore.response.StreamingBody.iter_chunks"
    ) {
        return Some(hit(&BuiltinSink::finding(
            "botocore.response.StreamingBody.read",
            RULE_BLOCKING_HTTP,
            "Use `aiobotocore` async response streams, or offload.",
        )));
    }
    // AWS-op name heuristic only on client/resource receivers — never
    // `botocore.session.*` / ComponentLocator / other internals (`get_*`).
    let is_api = q.ends_with("._make_api_call")
        || q.contains("make_api_call")
        || (is_boto_client_or_resource_api_qual(q) && method_looks_like_aws_op(q));
    if is_api {
        return Some(hit(&BuiltinSink::finding(
            "botocore.client.BaseClient._make_api_call",
            RULE_BLOCKING_HTTP,
            "Use `aioboto3` / `aiobotocore` instead of sync `boto3`.",
        )));
    }
    None
}

/// Strip a trailing `-stubs` from the top-level package segment without
/// allocating when no `-stubs` suffix is present.
pub(in crate::third_party) fn normalize_stubs_package_root_cow(qualname: &str) -> Cow<'_, str> {
    let Some((root, rest)) = qualname.split_once('.') else {
        return Cow::Borrowed(qualname.strip_suffix("-stubs").unwrap_or(qualname));
    };
    match root.strip_suffix("-stubs") {
        Some(stripped) => Cow::Owned(format!("{stripped}.{rest}")),
        None => Cow::Borrowed(qualname),
    }
}

/// Strip a trailing `-stubs` from the top-level package segment.
///
/// `boto3-stubs.session.Session.client` → `boto3.session.Session.client`,
/// `aioboto3-stubs.session.Session` → `aioboto3.session.Session`.
pub fn normalize_stubs_package_root(qualname: &str) -> String {
    normalize_stubs_package_root_cow(qualname).into_owned()
}

/// True for sync `boto3` / `botocore` package roots (after `-stubs` normalize).
pub fn is_sync_boto_package_qual(qualname: &str) -> bool {
    let q = normalize_stubs_package_root_cow(qualname);
    if is_aioboto_package_qual(&q) {
        return false;
    }
    q == "boto3" || q == "botocore" || q.starts_with("boto3.") || q.starts_with("botocore.")
}

/// True for `aioboto3` / `aiobotocore` roots (including `*-stubs` packages).
pub fn is_aioboto_package_qual(qualname: &str) -> bool {
    let q = normalize_stubs_package_root_cow(qualname);
    q == "aioboto3"
        || q == "aiobotocore"
        || q.starts_with("aioboto3.")
        || q.starts_with("aiobotocore.")
}

/// Sync `boto3.client` / `Session.client` factories — never aioboto3.
pub fn is_sync_boto_factory_qualname(qualname: &str) -> bool {
    let q = normalize_stubs_package_root_cow(qualname);
    if is_aioboto_package_qual(&q) {
        return false;
    }
    matches!(
        q.as_ref(),
        "boto3.client"
            | "boto3.resource"
            | "boto3.session.Session.client"
            | "boto3.session.Session.resource"
            | "boto3.Session.client"
            | "boto3.Session.resource"
    ) || ((q.starts_with("boto3.") || q.starts_with("botocore."))
        && (q.ends_with(".Session.client") || q.ends_with(".Session.resource")))
}

/// Sync AWS client/resource API method names (boto3 has no per-op stubs).
/// Excludes meta helpers (`get_paginator`, `get_waiter`, …) that are not IO.
pub fn aws_op_method_name(method: &str) -> bool {
    if matches!(
        method,
        "get_paginator"
            | "get_waiter"
            | "can_paginate"
            | "generate_presigned_url"
            | "generate_presigned_post"
            | "get_available_regions"
            | "meta"
            | "exceptions"
            | "waiter_names"
            | "get_available_subresources"
    ) {
        return false;
    }
    matches!(
        method,
        "get_object"
            | "put_object"
            | "head_object"
            | "head_bucket"
            | "copy_object"
            | "upload_part"
            | "upload_part_copy"
            | "complete_multipart_upload"
            | "abort_multipart_upload"
            | "list_objects"
            | "list_objects_v2"
            | "delete_object"
            | "send_message"
            | "receive_message"
            | "publish"
            | "get_item"
            | "put_item"
            | "batch_get_item"
            | "batch_write_item"
            | "transact_get_items"
            | "transact_write_items"
            | "execute_statement"
            | "batch_execute_statement"
            | "query"
            | "scan"
            | "invoke"
            | "invoke_model"
            | "invoke_model_with_response_stream"
            | "converse"
            | "converse_stream"
            | "decrypt"
            | "encrypt"
            | "re_encrypt"
            | "generate_data_key"
            | "generate_data_key_without_plaintext"
            | "start_execution"
            | "describe_instances"
            | "download_file"
            | "upload_file"
            | "download_fileobj"
            | "upload_fileobj"
            | "wait"
    ) || method.starts_with("get_")
        || method.starts_with("put_")
        || method.starts_with("list_")
        || method.starts_with("create_")
        || method.starts_with("delete_")
        || method.starts_with("describe_")
        || method.starts_with("update_")
        || method.starts_with("assume_role")
        || method.starts_with("wait_until_")
}

/// True when `qualname` is a sync boto client or resource API surface.
///
/// Operation-name heuristics must not apply to `botocore.session.*`,
/// `ComponentLocator`, or other non-client internals — those expose many
/// `get_*` helpers that are local metadata, not HTTP.
pub fn is_boto_client_or_resource_api_qual(qualname: &str) -> bool {
    let q = normalize_stubs_package_root_cow(qualname);
    if is_aioboto_package_qual(&q) {
        return false;
    }
    if q.starts_with("botocore.client.") {
        return true;
    }
    if q.starts_with("boto3.resources.") {
        return true;
    }
    false
}

fn method_looks_like_aws_op(q: &str) -> bool {
    q.rsplit_once('.')
        .is_some_and(|(_, method)| aws_op_method_name(method))
}
