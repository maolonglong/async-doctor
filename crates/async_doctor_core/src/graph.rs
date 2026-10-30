//! Plain call-graph facts emitted by the ty adapter (no ty types leak here).

use crate::report::EntryLabel;

/// Stable index into [`ProjectFacts::functions`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FunctionId(pub u32);

impl FunctionId {
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

/// Condition tags attached to a call site / hop.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConditionTag {
    Unconditional,
    AllPaths,
    Loop,
    AsyncBranch,
    /// FastAPI/Starlette sync `def` route (threadpool) — presentation only.
    SyncEntry,
}

impl ConditionTag {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unconditional => "unconditional",
            Self::AllPaths => "all-paths",
            Self::Loop => "loop",
            Self::AsyncBranch => "async-branch",
            Self::SyncEntry => "sync_entry",
        }
    }
}

/// What a call resolves to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CalleeRef {
    /// Call into another first-party function.
    Project(FunctionId),
    /// Resolved external/stdlib/third-party callable by dotted qualified name
    /// (e.g. `time.sleep`, `subprocess.run`, `pathlib.Path.read_text`).
    External { qualname: String },
    /// Dynamic / unresolvable dispatch — never guess by name.
    Unresolved { reason: String },
    /// Offload boundary: the callable argument is *not* invoked on the event loop.
    Offload(OffloadKind),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OffloadKind {
    AsyncioToThread,
    RunInExecutor,
    AnyioToThread,
    StarletteThreadpool,
    ThreadPoolExecutor,
}

impl OffloadKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AsyncioToThread => "asyncio.to_thread",
            Self::RunInExecutor => "loop.run_in_executor",
            Self::AnyioToThread => "anyio.to_thread.run_sync",
            Self::StarletteThreadpool => "starlette.concurrency.run_in_threadpool",
            Self::ThreadPoolExecutor => "concurrent.futures.ThreadPoolExecutor",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CallKind {
    /// Ordinary call expression.
    Call,
    /// Implicit dunder from `with` / `async with`.
    WithEnter,
    WithExit,
    /// Implicit dunder from `for` / `async for`.
    ForIter,
    /// Property getter access that resolves to a callable / blocking property.
    PropertyGet,
}

/// One directed call-graph edge: caller → callee at a concrete site.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallEdge {
    pub caller: FunctionId,
    pub callee: CalleeRef,
    pub file: String,
    pub line: u32,
    /// Source text of the call (or `with`/`for` site).
    pub text: String,
    pub kind: CallKind,
    pub conditions: Vec<ConditionTag>,
    /// Ordinal among edges from this caller that share the same callee key
    /// (used for stable finding ids when the chain otherwise collides).
    pub occurrence: u32,
    /// When true, this edge is an offload *wrapper* call (e.g. `to_thread(...)`).
    /// The wrapper itself is not a blocking sink; callable args are not edges.
    pub is_offload_wrapper: bool,
}

/// A first-party function or method.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionFact {
    pub id: FunctionId,
    pub file: String,
    pub line: u32,
    /// Module-relative dotted path without the function (`pkg.mod`), or empty for scripts.
    pub module: String,
    /// Class-nested name (`Class.method`) or bare function name.
    pub qualname: String,
    pub is_async: bool,
    pub label: EntryLabel,
    /// Sync FastAPI/Starlette route handlers run in a threadpool — not entries.
    pub is_sync_route: bool,
    /// Async twins discovered on the same class (method name → twin name), for fix hints.
    pub async_twins: Vec<(String, String)>,
}

impl FunctionFact {
    /// `module:qualname` when module is non-empty, else just `qualname`.
    pub fn full_qualname(&self) -> String {
        if self.module.is_empty() {
            self.qualname.clone()
        } else {
            format!("{}:{}", self.module, self.qualname)
        }
    }
}

/// All facts for one project scan (first-party only for M1).
#[derive(Debug, Clone, Default)]
pub struct ProjectFacts {
    pub functions: Vec<FunctionFact>,
    pub edges: Vec<CallEdge>,
    pub complete: bool,
    pub skipped: Vec<String>,
}

impl ProjectFacts {
    pub fn function(&self, id: FunctionId) -> &FunctionFact {
        &self.functions[id.index()]
    }
}
