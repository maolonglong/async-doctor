//! Built-in rule / sink tables for async-doctor.
//!
//! Stdlib sinks plus third-party type-based rules.

mod async_twin;
mod docs;
mod stdlib;
mod third_party;

pub use async_twin::{
    ASYNC_TWINS, async_twin_for_sink, is_excluded_from_twin_advice, twin_resolution_symbols,
};
pub use docs::{DOCS_RULES_BASE, RULE_DOCS, RuleDoc, docs_anchor_for, docs_url_for, rule_doc};
pub use stdlib::{STDLIB_SINKS, StdlibSinkTable};
pub use third_party::{
    BuiltinSinkTable, THIRD_PARTY_SINKS, aws_op_method_name, is_aioboto_package_qual,
    is_async_counterpart, is_boto_client_or_resource_api_qual, is_sync_boto_factory_qualname,
    is_sync_boto_package_qual, normalize_stubs_package_root,
};

pub const RULE_OFFLOAD_HAS_ASYNC_TWIN: &str = "offload-has-async-twin";
pub const RULE_NESTED_EVENT_LOOP: &str = "nested-event-loop";
pub const RULE_OFFLOAD_COROUTINE: &str = "offload-coroutine";
pub const RULE_SYNC_WITH_ASYNC_CM: &str = "sync-with-async-cm";
pub const RULE_SYNC_FOR_ASYNC_ITERABLE: &str = "sync-for-async-iterable";
pub const RULE_EAGER_OFFLOAD_CALL: &str = "eager-offload-call";
pub const RULE_UNAWAITED_COROUTINE: &str = "unawaited-coroutine";
pub const RULE_DANGLING_TASK: &str = "dangling-task";
pub const RULE_EPHEMERAL_ASYNC_CLIENT: &str = "ephemeral-async-client";
pub const RULE_SHARED_ASYNC_SESSION: &str = "shared-async-session";
pub const RULE_OFFLOAD_STATEFUL_SESSION: &str = "offload-stateful-session";
pub const RULE_USELESS_LOCAL_ASYNC_LOCK: &str = "useless-local-async-lock";
pub const RULE_SWALLOWED_CANCELLED_ERROR: &str = "swallowed-cancelled-error";

use async_doctor_core::SinkHit;

pub const RULE_BLOCKING_SLEEP: &str = "blocking-sleep";
pub const RULE_BLOCKING_SOCKET: &str = "blocking-socket";
pub const RULE_BLOCKING_HTTP: &str = "blocking-http";
pub const RULE_BLOCKING_SUBPROCESS: &str = "blocking-subprocess";
pub const RULE_BLOCKING_FILE: &str = "blocking-file";
pub const RULE_BLOCKING_LOCK: &str = "blocking-lock";
pub const RULE_BLOCKING_DB: &str = "blocking-db";
pub const RULE_BLOCKING_CPU: &str = "blocking-cpu";

/// One built-in sink primitive and its diagnostic classification.
///
/// Recognizing a sink does not establish a recommended repair.
#[derive(Debug, Clone, Copy)]
pub struct BuiltinSink {
    pub qualname: &'static str,
    pub rule: &'static str,
    pub candidate_only: bool,
    pub candidate_reason: Option<&'static str>,
    /// Only fire for `with` enter edges (e.g. `with Engine.begin()`).
    pub with_only: bool,
    /// Only fire when the call is consumed as a generator/iterator
    /// (e.g. `for msg in pubsub.listen()`).
    pub consume_only: bool,
}

impl BuiltinSink {
    pub const fn finding(qualname: &'static str, rule: &'static str) -> Self {
        Self {
            qualname,
            rule,
            candidate_only: false,
            candidate_reason: None,
            with_only: false,
            consume_only: false,
        }
    }

    pub const fn finding_with_only(qualname: &'static str, rule: &'static str) -> Self {
        Self {
            qualname,
            rule,
            candidate_only: false,
            candidate_reason: None,
            with_only: true,
            consume_only: false,
        }
    }

    pub const fn finding_consume_only(qualname: &'static str, rule: &'static str) -> Self {
        Self {
            qualname,
            rule,
            candidate_only: false,
            candidate_reason: None,
            with_only: false,
            consume_only: true,
        }
    }

    pub const fn candidate(
        qualname: &'static str,
        rule: &'static str,
        reason: &'static str,
    ) -> Self {
        Self {
            qualname,
            rule,
            candidate_only: true,
            candidate_reason: Some(reason),
            with_only: false,
            consume_only: false,
        }
    }

    /// Build a diagnostic hit with no repair recommendation (`fix_hint` is empty).
    pub fn hit(self) -> SinkHit {
        SinkHit {
            rule: self.rule,
            sink_qualname: self.qualname,
            fix_hint: "",
            candidate_only: self.candidate_only,
            candidate_reason: self.candidate_reason,
            with_only: self.with_only,
            consume_only: self.consume_only,
        }
    }
}
