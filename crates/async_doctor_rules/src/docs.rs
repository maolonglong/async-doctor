//! Long-form rule help for `async-doctor explain` and docs anchors.
//!
//! Absolute docs URLs point at the public GitHub tree so installed binaries
//! still have a clickable link.

/// Canonical rules document on GitHub (main branch).
pub const DOCS_RULES_BASE: &str =
    "https://github.com/maolonglong/async-doctor/blob/main/docs/rules.md";

/// Stable documentation for one built-in rule.
#[derive(Debug, Clone, Copy)]
pub struct RuleDoc {
    pub id: &'static str,
    pub title: &'static str,
    /// One-line summary for agents and compact CLI output.
    pub summary: &'static str,
    /// Longer explanation: why it blocks the event loop and how to fix it.
    pub body: &'static str,
    /// Typical fix patterns (async twin, `to_thread`, client swap).
    pub fix: &'static str,
}

impl RuleDoc {
    /// Markdown / URL fragment, e.g. `#blocking-sleep`.
    pub fn docs_anchor(&self) -> String {
        format!("#{}", self.id)
    }

    /// Absolute docs URL for agents and installed binaries.
    pub fn docs_url(&self) -> String {
        format!("{DOCS_RULES_BASE}{}", self.docs_anchor())
    }

    /// Full text printed by `async-doctor explain <rule-id>`.
    pub fn explain_text(&self) -> String {
        format!(
            "{id} — {title}\n\n{summary}\n\nWhy it matters\n{body}\n\nHow to fix\n{fix}\n\nDocs: {url}\n",
            id = self.id,
            title = self.title,
            summary = self.summary,
            body = self.body,
            fix = self.fix,
            url = self.docs_url(),
        )
    }
}

/// Built-in rules that agents and `explain` can look up.
pub static RULE_DOCS: &[RuleDoc] = &[
    RuleDoc {
        id: "blocking-sleep",
        title: "Blocking sleep on the event loop",
        summary: "`time.sleep` (and similar) freezes every task sharing the loop.",
        body: "\
While `time.sleep` runs, the asyncio event loop cannot schedule other coroutines,\n\
so latency spikes and timeouts pile up across the process. Prefer the async twin\n\
`asyncio.sleep`, or offload truly sync work with `asyncio.to_thread`.",
        fix: "\
- Await `asyncio.sleep(...)` instead of `time.sleep(...)`.\n\
- Or offload: `await asyncio.to_thread(time.sleep, seconds)`.",
    },
    RuleDoc {
        id: "blocking-socket",
        title: "Blocking socket / DNS / SSL",
        summary: "Sync socket, `getaddrinfo`, or SSL handshake stalls the loop.",
        body: "\
BSD sockets and blocking DNS resolve on the calling thread. Inside `async def`,\n\
that thread is the event loop. Use asyncio stream/server helpers, or run the\n\
sync call in a worker thread.",
        fix: "\
- Prefer `asyncio.open_connection` / asyncio servers / `loop.getaddrinfo`.\n\
- Or `await asyncio.to_thread(socket.create_connection, ...)`.",
    },
    RuleDoc {
        id: "blocking-http",
        title: "Blocking HTTP client",
        summary: "Sync HTTP (`urllib`, `requests`, sync `httpx`) blocks the loop.",
        body: "\
HTTP round-trips are often tens to hundreds of milliseconds. A sync client holds\n\
the loop for the whole wait. Switch to an async client, or offload the call.",
        fix: "\
- Use `httpx.AsyncClient`, `aiohttp`, or another async HTTP API.\n\
- Or `await asyncio.to_thread(requests.get, url)`.",
    },
    RuleDoc {
        id: "blocking-subprocess",
        title: "Blocking subprocess",
        summary: "`subprocess.run` / `Popen.wait` / `os.system` block until exit.",
        body: "\
Waiting on a child process from the event-loop thread prevents other tasks from\n\
running for the entire child lifetime.",
        fix: "\
- Prefer `asyncio.create_subprocess_exec` / `create_subprocess_shell` and await\n\
  `communicate` / `wait`.\n\
- Or offload the sync subprocess helper with `asyncio.to_thread`.",
    },
    RuleDoc {
        id: "blocking-file",
        title: "Blocking file / path IO",
        summary: "`open`, path reads/writes, and directory walks block on disk.",
        body: "\
Disk IO can stall under load or on network filesystems. Inside `async def`, use\n\
async file helpers or offload sync filesystem calls.",
        fix: "\
- Use `aiofiles` / AnyIO file APIs where practical.\n\
- Or `await asyncio.to_thread(path.read_text)` / `to_thread(open, ...)`.",
    },
    RuleDoc {
        id: "blocking-lock",
        title: "Blocking thread lock / queue / future",
        summary: "Threading locks, `queue.Queue`, and `Future.result` can stall the loop.",
        body: "\
A contended lock or blocking queue get on the event-loop thread freezes every\n\
task. Prefer asyncio primitives, or confine blocking waits to worker threads.",
        fix: "\
- Prefer `asyncio.Lock` / `Event` / `Queue`.\n\
- Await `asyncio.wrap_future` instead of `Future.result()`.\n\
- Hold threading locks only inside `to_thread` workers.",
    },
    RuleDoc {
        id: "blocking-db",
        title: "Blocking database / client SDK",
        summary: "Sync DB drivers and cloud/AI SDKs block on network round-trips.",
        body: "\
Sync SQL/Redis/HTTP-based SDKs perform IO on the calling thread. From `async def`,\n\
that blocks the loop for every query. Prefer async drivers/clients, or offload.",
        fix: "\
- Switch to the async client twin when one exists (`AsyncSession`, `redis.asyncio`,\n\
  `AsyncOpenAI`, …).\n\
- Or wrap sync calls: `await asyncio.to_thread(session.execute, ...)`.",
    },
    RuleDoc {
        id: "unresolved-call",
        title: "Unresolved call (candidate only)",
        summary: "Dynamic dispatch could not be proven; reported as a candidate.",
        body: "\
When the callee cannot be resolved statically, async-doctor does not invent a\n\
finding. Candidates need human or agent judgment and never fail the exit code\n\
by themselves.",
        fix: "\
- Inspect the call; add types or simplify indirection so the sink is visible.\n\
- If it is truly sync IO, await an async API or offload with `to_thread`.",
    },
];

/// Look up a rule by id (`blocking-sleep`) or docs anchor (`#blocking-sleep`).
pub fn rule_doc(id_or_anchor: &str) -> Option<&'static RuleDoc> {
    let id = id_or_anchor.strip_prefix('#').unwrap_or(id_or_anchor);
    RULE_DOCS.iter().find(|r| r.id == id)
}

/// Docs anchor string for a rule id (always `#<id>`, even if unknown).
pub fn docs_anchor_for(rule: &str) -> String {
    match rule_doc(rule) {
        Some(doc) => doc.docs_anchor(),
        None => format!("#{rule}"),
    }
}

/// Absolute docs URL for a rule id.
pub fn docs_url_for(rule: &str) -> String {
    match rule_doc(rule) {
        Some(doc) => doc.docs_url(),
        None => format!("{DOCS_RULES_BASE}#{rule}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_stdlib_rule_constant_has_docs() {
        use crate::{
            RULE_BLOCKING_DB, RULE_BLOCKING_FILE, RULE_BLOCKING_HTTP, RULE_BLOCKING_LOCK,
            RULE_BLOCKING_SLEEP, RULE_BLOCKING_SOCKET, RULE_BLOCKING_SUBPROCESS,
        };
        for id in [
            RULE_BLOCKING_SLEEP,
            RULE_BLOCKING_SOCKET,
            RULE_BLOCKING_HTTP,
            RULE_BLOCKING_SUBPROCESS,
            RULE_BLOCKING_FILE,
            RULE_BLOCKING_LOCK,
            RULE_BLOCKING_DB,
        ] {
            assert!(rule_doc(id).is_some(), "missing RuleDoc for {id}");
        }
    }

    #[test]
    fn explain_mentions_fix_and_absolute_url() {
        let text = rule_doc("blocking-sleep").unwrap().explain_text();
        assert!(text.contains("asyncio.sleep"));
        assert!(text.contains("#blocking-sleep"));
        assert!(
            text.contains("https://github.com/maolonglong/async-doctor/blob/main/docs/rules.md")
        );
    }
}
