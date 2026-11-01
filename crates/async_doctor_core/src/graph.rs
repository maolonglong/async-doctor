//! Plain call-graph facts emitted by the ty adapter (no ty types leak here).

use crate::environment::ScanEnvironment;
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
///
/// Only emit a tag you can prove. `Unconditional` / `AllPaths` must not appear
/// alongside any branch/try/match guard on the same hop or merged chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConditionTag {
    Unconditional,
    AllPaths,
    Loop,
    /// Call sits under an `if`/`else`/`try`/`match` (or similar) guard.
    AsyncBranch,
    /// Call sits behind a module/global cache miss check (`if _cache is None`).
    CacheMissDependent,
    /// Call sits under a "no running event loop" guard (`if loop is None`,
    /// `except RuntimeError` after `get_running_loop`, `not loop.is_running()`, …).
    /// Nested-event-loop seeds with this tag are dropped (runtime-safe when async).
    NoRunningLoopGuard,
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
            Self::CacheMissDependent => "cache_miss_dependent",
            Self::NoRunningLoopGuard => "no_running_loop_guard",
            Self::SyncEntry => "sync_entry",
        }
    }

    pub fn is_branch_like(self) -> bool {
        matches!(
            self,
            Self::AsyncBranch | Self::CacheMissDependent | Self::NoRunningLoopGuard
        )
    }
}

/// Proven constant value of a positional call argument (when known).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LiteralArg {
    True,
    False,
    None,
}

/// What a call resolves to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CalleeRef {
    /// Call into another first-party (or followed third-party) function.
    Project(FunctionId),
    /// Resolved external/stdlib/third-party callable by dotted qualified name
    /// (e.g. `time.sleep`, `subprocess.run`, `pathlib.Path.read_text`).
    External { qualname: String },
    /// Dynamic / unresolvable dispatch — never guess by name.
    Unresolved { reason: String },
    /// Offload boundary: the callable argument is *not* invoked on the event loop.
    Offload(OffloadKind),
    /// Call of a positional parameter (higher-order): resolve via caller args.
    ParamInvoke { index: u32 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OffloadKind {
    AsyncioToThread,
    RunInExecutor,
    AnyioToThread,
    StarletteThreadpool,
    ThreadPoolExecutor,
    /// `asgiref.sync.sync_to_async` — Django/ASGI sync→async offload boundary.
    AsgirefSyncToAsync,
}

impl OffloadKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AsyncioToThread => "asyncio.to_thread",
            Self::RunInExecutor => "loop.run_in_executor",
            Self::AnyioToThread => "anyio.to_thread.run_sync",
            Self::StarletteThreadpool => "starlette.concurrency.run_in_threadpool",
            Self::ThreadPoolExecutor => "concurrent.futures.ThreadPoolExecutor",
            Self::AsgirefSyncToAsync => "asgiref.sync.sync_to_async",
        }
    }

    /// Positional argument index of the offloaded callable.
    ///
    /// `run_in_executor(executor, func, …)` takes `func` at index 1; every other
    /// built-in offload takes it at index 0.
    pub fn callable_arg_index(self) -> usize {
        match self {
            Self::RunInExecutor => 1,
            _ => 0,
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
    /// Consuming a generator (for/next/list/yield from) — body may run.
    GeneratorConsume,
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
    /// Positional argument literals when known (`None` = unknown / non-literal).
    pub positional_literals: Vec<Option<LiteralArg>>,
    /// Positional arguments that resolve to project/followed functions (higher-order).
    pub positional_callables: Vec<Option<FunctionId>>,
    /// Positional arguments that resolve to external qualnames (offload targets
    /// that are typed third-party/stdlib callables rather than project defs).
    pub positional_externals: Vec<Option<String>>,
    /// For positional callable args: root constructed client class when known
    /// (`openai.AzureOpenAI` for `client.chat.completions.create` on an Azure client).
    /// Used so twin advice prefers `AsyncAzureOpenAI` over the shared resources path.
    pub positional_constructed_receivers: Vec<Option<String>>,
    /// Keyword arguments that resolve to project functions (`fn=block`, including
    /// `partial` / bound methods). Cheap call-context for higher-order kwargs.
    pub keyword_callables: Vec<(String, FunctionId)>,
    /// When set, this edge only executes when positional parameter `index` is
    /// truthy (`if param:`) or falsy (`if not param:` when `inverted`).
    pub param_guard: Option<ParamGuard>,
    /// True when this call is the *direct* operand of `await` / `async for` /
    /// `async with`. Nested argument calls stay `false`.
    ///
    /// Awaited *async* APIs are not sync-IO sinks; awaited *sync* sinks still
    /// run on the event loop before `await` and must be reported.
    pub is_awaited: bool,
    /// Whether the resolved callee definition is an `async def` (coroutine).
    /// `Some(true)` / `Some(false)` from ty (including through MRO / `super`);
    /// `None` when unresolved — name-based async-counterpart rules are the
    /// fallback only in that case.
    pub callee_is_coroutine: Option<bool>,
    /// True when this call is an argument to an offload wrapper and was
    /// *evaluated* (eager `to_thread(f())`) rather than passed as a callable.
    pub is_eager_offload_arg: bool,
    /// Higher-order materialization: this edge (`apply → block`) is only valid
    /// when the previous hop’s caller is this function (the call site that
    /// passed `block`). Cheap call-context sensitivity without cloning graphs.
    pub context_from_caller: Option<FunctionId>,
    /// Root constructed client class for an attribute callee
    /// (`AzureOpenAI` for `AzureOpenAI().chat.completions.create(...)`).
    pub constructed_receiver: Option<String>,
}

/// Parameter-controlled branch guarding a call site.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ParamGuard {
    /// Positional parameter index in the enclosing function.
    pub index: u32,
    /// When true, the branch is `if not param:` (taken when param is falsy).
    pub inverted: bool,
}

/// A first-party or followed third-party function or method.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionFact {
    pub id: FunctionId,
    pub file: String,
    pub line: u32,
    /// Module-relative dotted path without the function (`pkg.mod`), or empty for scripts.
    pub module: String,
    /// Scope-qualified name: `Class.method`, `outer.inner` for nested defs, or bare name.
    pub qualname: String,
    pub is_async: bool,
    /// True when the body contains `yield` / `yield from` (generator function).
    pub is_generator: bool,
    pub label: EntryLabel,
    /// Sync FastAPI/Starlette route handlers run in a threadpool — not entries.
    pub is_sync_route: bool,
    /// Async twins discovered on the same class (method name → twin name), for fix hints.
    pub async_twins: Vec<(String, String)>,
    /// Positional parameter names (for guard matching / higher-order).
    pub param_names: Vec<String>,
    /// True when this function was extracted from site-packages (not first-party).
    pub is_third_party: bool,
    /// Distribution name when `is_third_party` (e.g. `requests`).
    pub package_name: Option<String>,
    /// Distribution version when known (e.g. `2.32.3`).
    pub package_version: Option<String>,
    /// Positional parameter indices that this function *calls* (higher-order).
    pub invokes_params: Vec<u32>,
    /// Inferred offload wrapper: body passes a callable param into a real offload primitive.
    pub is_offload_wrapper: bool,
}

impl FunctionFact {
    /// Installed site-/dist-packages follow — not outside-editable follow-only.
    ///
    /// Outside editables are `is_third_party` (never entries) but keep first-party
    /// grading for branch sinks: chains into them from the scan root still find.
    pub fn is_installed_third_party(&self) -> bool {
        self.is_third_party && !self.file.replace('\\', "/").starts_with("editable/")
    }

    /// `module:qualname` when module is non-empty, else just `qualname`.
    pub fn full_qualname(&self) -> String {
        if self.module.is_empty() {
            self.qualname.clone()
        } else {
            format!("{}:{}", self.module, self.qualname)
        }
    }
}

/// Options controlling grading / reporting (CLI flags; no config file).
#[derive(Debug, Clone, Copy, Default)]
pub struct AnalyzeOptions {
    /// Include entries under tests/docs/examples/scripts/hidden dirs.
    pub include_non_production: bool,
    /// Include `reason=unresolved` candidates in the report.
    /// When set, unresolved sites are aggregated by callee pattern and ranked
    /// by number of entries affected (see [`crate::Report::unresolved_summary`]).
    pub show_unresolved: bool,
}

/// A first-party import that ty could not resolve (one site, never per-call).
///
/// Deduped per `(file, module)` — same missing module imported twice in one
/// file counts once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnresolvedImport {
    pub file: String,
    pub line: u32,
    /// Imported module key (e.g. `libcore.cfg`, `missing_pkg`).
    pub module: String,
    pub text: String,
}

/// All facts for one project scan (first-party + reachable third-party for M2).
#[derive(Debug, Clone, Default)]
pub struct ProjectFacts {
    pub functions: Vec<FunctionFact>,
    pub edges: Vec<CallEdge>,
    pub complete: bool,
    pub skipped: Vec<String>,
    /// Directory scan found no `.py` files (empty dir or non-Python tree).
    pub no_python_files: bool,
    /// Failed first-party `import` / `from … import` sites (deduped by
    /// file+module). Only when the head module exists under the scan root /
    /// extra roots yet still fails to resolve.
    pub unresolved_imports: Vec<UnresolvedImport>,
    /// Unique top-level third-party module names that failed to import
    /// (sorted). Surfaced as a single aggregated note, never per-import.
    pub unresolved_third_party_modules: Vec<String>,
    /// Unique top-level third-party module names that resolved successfully
    /// (sorted). Used with unresolved counts for the environment advisory ratio.
    pub resolved_third_party_modules: Vec<String>,
    /// Twin / dependency symbols that resolve in the scanned venv (populated
    /// during extract). Used by advice rules that require the async twin (and
    /// its `needs`) to be installed.
    pub resolved_symbols: std::collections::BTreeSet<String>,
    /// Interpreter / venv discovered for this extract (copied onto the report).
    pub environment: ScanEnvironment,
}

impl ProjectFacts {
    pub fn function(&self, id: FunctionId) -> &FunctionFact {
        &self.functions[id.index()]
    }
}

/// Return true when an entry file should be omitted by default (tests/docs/…).
pub fn is_non_production_entry_path(file: &str) -> bool {
    let f = file.replace('\\', "/");
    let name = f.rsplit('/').next().unwrap_or(&f);
    if name.starts_with('.') {
        return true;
    }
    if name == "conftest.py" {
        return true;
    }
    if name.starts_with("test_") || name.ends_with("_test.py") {
        return true;
    }
    for part in f.split('/') {
        if part.is_empty() {
            continue;
        }
        if part.starts_with('.') {
            return true;
        }
        match part {
            "tests" | "test" | "docs" | "doc" | "examples" | "example" | "scripts" | "script"
            | "benchmarks" | "benchmark" => return true,
            _ => {}
        }
    }
    false
}
