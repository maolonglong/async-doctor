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

use async_doctor_core::SinkHit;

pub const RULE_BLOCKING_SLEEP: &str = "blocking-sleep";
pub const RULE_BLOCKING_SOCKET: &str = "blocking-socket";
pub const RULE_BLOCKING_HTTP: &str = "blocking-http";
pub const RULE_BLOCKING_SUBPROCESS: &str = "blocking-subprocess";
pub const RULE_BLOCKING_FILE: &str = "blocking-file";
pub const RULE_BLOCKING_LOCK: &str = "blocking-lock";
pub const RULE_BLOCKING_DB: &str = "blocking-db";

pub fn blocking_sleep_fix_hint() -> &'static str {
    "Replace `time.sleep(...)` with `await asyncio.sleep(...)` \
     (or offload with `await asyncio.to_thread(...)`)."
}

/// One stdlib (or builtins) sink primitive.
#[derive(Debug, Clone, Copy)]
pub struct BuiltinSink {
    pub qualname: &'static str,
    pub rule: &'static str,
    pub fix_hint: &'static str,
    pub candidate_only: bool,
    pub candidate_reason: Option<&'static str>,
    /// Only fire for `with` enter edges (e.g. `with Engine.begin()`).
    pub with_only: bool,
    /// Only fire when the call is consumed as a generator/iterator
    /// (e.g. `for msg in pubsub.listen()`).
    pub consume_only: bool,
}

impl BuiltinSink {
    pub const fn finding(
        qualname: &'static str,
        rule: &'static str,
        fix_hint: &'static str,
    ) -> Self {
        Self {
            qualname,
            rule,
            fix_hint,
            candidate_only: false,
            candidate_reason: None,
            with_only: false,
            consume_only: false,
        }
    }

    pub const fn finding_with_only(
        qualname: &'static str,
        rule: &'static str,
        fix_hint: &'static str,
    ) -> Self {
        Self {
            qualname,
            rule,
            fix_hint,
            candidate_only: false,
            candidate_reason: None,
            with_only: true,
            consume_only: false,
        }
    }

    pub const fn finding_consume_only(
        qualname: &'static str,
        rule: &'static str,
        fix_hint: &'static str,
    ) -> Self {
        Self {
            qualname,
            rule,
            fix_hint,
            candidate_only: false,
            candidate_reason: None,
            with_only: false,
            consume_only: true,
        }
    }

    pub const fn candidate(
        qualname: &'static str,
        rule: &'static str,
        fix_hint: &'static str,
        reason: &'static str,
    ) -> Self {
        Self {
            qualname,
            rule,
            fix_hint,
            candidate_only: true,
            candidate_reason: Some(reason),
            with_only: false,
            consume_only: false,
        }
    }

    pub fn hit(self) -> SinkHit {
        SinkHit {
            rule: self.rule,
            sink_qualname: self.qualname,
            fix_hint: self.fix_hint,
            candidate_only: self.candidate_only,
            candidate_reason: self.candidate_reason,
            with_only: self.with_only,
            consume_only: self.consume_only,
        }
    }
}
