//! Built-in rule metadata. M0 ships a single direct-blocking rule.

use async_doctor_core::Fix;

pub const RULE_BLOCKING_SLEEP: &str = "blocking-sleep";

pub fn blocking_sleep_fix() -> Fix {
    Fix {
        hint: "Replace `time.sleep(...)` with `await asyncio.sleep(...)` \
               (or offload with `await asyncio.to_thread(...)`)."
            .into(),
        // Filled by the scanner with a stable group id; placeholder replaced there.
        fix_group_id: String::new(),
    }
}

pub fn blocking_sleep_fix_hint() -> &'static str {
    "Replace `time.sleep(...)` with `await asyncio.sleep(...)` \
     (or offload with `await asyncio.to_thread(...)`)."
}
