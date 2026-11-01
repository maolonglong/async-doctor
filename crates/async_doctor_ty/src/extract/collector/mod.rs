//! Call-graph edge collection over a single Python file.

mod calls;
mod control_flow;
mod imports;
pub(crate) mod loop_guards;
mod nonblocking;
mod redis_mro;
mod resolve;
mod visit;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use async_doctor_core::{FunctionId, LiteralArg, ParamGuard, ProjectFacts};
use ruff_python_ast::{self as ast};
use ty_python_semantic::SemanticModel;

use crate::sitepkgs::{FollowTarget, PackageMetaMemo};

use super::constants::ModuleConst;
use super::ctx::{DefKey, FileCtx, TaskTargets};

pub(crate) struct CallCollector<'a, 'db> {
    // Fields are `pub(super)` so `extract::{files,follow}` can fill the struct
    // literal; collector child modules already see them as same-module private.
    pub(super) ctx: &'a FileCtx<'db>,
    pub(super) model: &'a SemanticModel<'db>,
    pub(super) facts: &'a mut ProjectFacts,
    pub(super) def_map: &'a mut HashMap<DefKey, FunctionId>,
    /// Optional shared base for parallel pass-2: lookups fall through here so
    /// `def_map` only holds this file's inserts (no per-file full-map clone).
    pub(super) def_map_base: Option<&'a HashMap<DefKey, FunctionId>>,
    pub(super) qual_map: &'a mut HashMap<(String, String), FunctionId>,
    pub(super) qual_map_base: Option<&'a HashMap<(String, String), FunctionId>>,
    pub(super) property_defs: &'a HashSet<DefKey>,
    pub(super) task_targets: &'a mut TaskTargets,
    pub(super) follow_targets: &'a mut HashMap<String, FollowTarget>,
    pub(super) package_meta: &'a mut PackageMetaMemo,
    /// Outside-scan-root editable roots — followable like site-packages.
    pub(super) followable_extra_roots: &'a [PathBuf],
    /// When false (third-party follow), skip unresolved-import recording.
    pub(super) record_unresolved_imports: bool,
    pub(super) module_constants: &'a HashMap<String, ModuleConst>,
    pub(super) current_fn: Option<FunctionId>,
    pub(super) current_param_names: Vec<String>,
    pub(super) class_stack: Vec<String>,
    pub(super) fn_stack: Vec<String>,
    pub(super) dead_depth: u32,
    pub(super) loop_depth: u32,
    pub(super) branch_depth: u32,
    pub(super) cache_miss_depth: u32,
    /// Depth under a "no running event loop" guard (see [`ConditionTag::NoRunningLoopGuard`]).
    pub(super) no_running_loop_depth: u32,
    /// Try-body saw `get_running_loop` / `get_event_loop` (for except-RuntimeError tagging).
    pub(super) try_saw_get_loop: bool,
    pub(super) active_param_guard: Option<ParamGuard>,
    /// True while visiting an expression that consumes a generator.
    pub(super) consuming_generator: bool,
    pub(super) in_lambda: bool,
    /// When true, the enclosing lambda was passed to an offload boundary.
    pub(super) skip_lambda_body: bool,
    /// True while visiting `call.func` — suppress PropertyGet on that Attribute.
    pub(super) in_callee_position: bool,
    /// When true, the *next* recorded Call edge is the direct operand of
    /// `await` / `async for` iter / `async with` context (nested arg calls are not).
    pub(super) pending_is_awaited: bool,
    /// When true, the next Call edge is a `with` context Call (e.g. `Engine.begin()`).
    pub(super) pending_with_enter: bool,
    /// True while visiting an eagerly-evaluated offload argument (`to_thread(f())`).
    pub(super) in_eager_offload_arg: bool,
    /// Locals bound by `async with <asyncio-redis> as name` → receiver type qual.
    pub(super) async_with_receivers: HashMap<String, String>,
    /// Memo: receiver type qualname → redis client family (avoids repeat MRO walks).
    pub(super) redis_family_memo:
        HashMap<String, Option<crate::extract::helpers::RedisClientFamily>>,
    pub(super) occurrence_counter: HashMap<(FunctionId, String), u32>,
    /// Literals for the call currently being recorded (set by handle_call).
    pub(super) pending_literals: Vec<Option<LiteralArg>>,
    /// Positional callables for the call currently being recorded.
    pub(super) pending_callables: Vec<Option<FunctionId>>,
    /// Positional external qualnames for the call currently being recorded.
    pub(super) pending_externals: Vec<Option<String>>,
    /// Positional constructed client roots for the call currently being recorded.
    pub(super) pending_constructed_receivers: Vec<Option<String>>,
    /// Constructed client root for the callee attribute chain (when recording).
    pub(super) pending_constructed_receiver: Option<String>,
    /// Keyword callables for the call currently being recorded.
    pub(super) pending_keyword_callables: Vec<(String, FunctionId)>,
    /// Cheap type narrowing from `isinstance` / `is not None` in the current branch.
    pub(super) narrowed_locals: HashMap<String, crate::extract::helpers::NarrowFact>,
    /// Local name → lambda assigned in the current function.
    pub(super) local_lambdas: HashMap<String, &'a ast::ExprLambda>,
    /// Local name → generator expression assigned in the current function.
    pub(super) local_genexprs: HashMap<String, &'a ast::ExprGenerator>,
    /// Locals bound only when no event loop is running (ternary / idle-loop helper).
    /// `if name is not None:` (or truthy `if name:`) is a [`ConditionTag::NoRunningLoopGuard`].
    pub(super) idle_loop_locals: HashSet<String>,
    /// Function/method names whose body returns a loop only when it is not running.
    pub(super) idle_loop_helpers: HashSet<String>,
    /// Names bound to an unbounded `queue.Queue()` / `Queue(maxsize<=0)`.
    /// `put` on these never blocks.
    pub(super) unbounded_queues: HashSet<String>,
    /// Locals assigned from `asyncio.get_running_loop()` (incl. `= None` after
    /// `except RuntimeError`). Early-return `if name is not None` only guards
    /// when the name is in this set.
    pub(super) running_loop_locals: HashSet<String>,
    /// Locals assigned from `asyncio.get_event_loop()`. Early-return only when
    /// combined with `.is_running()` (not bare `is not None`).
    pub(super) event_loop_locals: HashSet<String>,
}

impl<'a, 'db> CallCollector<'a, 'db> {
    pub(super) fn def_get(&self, key: &DefKey) -> Option<FunctionId> {
        self.def_map
            .get(key)
            .copied()
            .or_else(|| self.def_map_base.and_then(|b| b.get(key).copied()))
    }

    pub(super) fn qual_get(&self, key: &(String, String)) -> Option<FunctionId> {
        self.qual_map
            .get(key)
            .copied()
            .or_else(|| self.qual_map_base.and_then(|b| b.get(key).copied()))
    }
}
