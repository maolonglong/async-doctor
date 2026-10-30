//! Built-in rule metadata. M0 ships a single direct-blocking rule.

pub const RULE_BLOCKING_SLEEP: &str = "blocking-sleep";

pub fn blocking_sleep_fix_hint() -> &'static str {
    "Replace `time.sleep(...)` with `await asyncio.sleep(...)` \
     (or offload with `await asyncio.to_thread(...)`)."
}
