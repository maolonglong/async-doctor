//! Family / heuristic matchers for third-party sync APIs.

mod aws;
mod http_ai;
mod kv;
mod sql_doc;

use async_doctor_core::SinkHit;

pub(super) use aws::normalize_stubs_package_root_cow;
pub use aws::{
    aws_op_method_name, is_aioboto_package_qual, is_boto_client_or_resource_api_qual,
    is_sync_boto_factory_qualname, is_sync_boto_package_qual, normalize_stubs_package_root,
};
pub(super) use kv::is_redis_pipeline_buffering;

pub(super) fn match_family(q: &str) -> Option<SinkHit> {
    if let Some(hit) = http_ai::match_http_ai_family(q) {
        return Some(hit);
    }
    if let Some(res) = kv::match_kv_family(q) {
        return res;
    }
    if let Some(hit) = sql_doc::match_sql_or_doc_family(q) {
        return Some(hit);
    }
    aws::match_aws_family(q)
}

pub(super) fn intern_qualname(s: &str) -> &'static str {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    static CACHE: OnceLock<Mutex<HashMap<String, &'static str>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut guard = cache.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(&existing) = guard.get(s) {
        return existing;
    }
    let leaked: &'static str = Box::leak(s.to_owned().into_boxed_str());
    guard.insert(s.to_owned(), leaked);
    leaked
}
