//! Shared panic payload formatting for per-file isolation.

use std::any::Any;

/// Human-readable reason from a `catch_unwind` payload.
pub(crate) fn panic_message(payload: &Box<dyn Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        format!("analysis panicked: {s}")
    } else if let Some(s) = payload.downcast_ref::<String>() {
        format!("analysis panicked: {s}")
    } else {
        "analysis panicked".to_string()
    }
}
