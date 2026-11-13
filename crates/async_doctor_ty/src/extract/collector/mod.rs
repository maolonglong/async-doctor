//! Call-graph edge collection over a single Python file.

mod async_anti_patterns;
mod boto;
mod buffers;
mod calls;
mod concurrency_anti_patterns;
mod constructors;
mod control_flow;
mod eager_offload;
mod imports;
mod locals;
pub(crate) mod loop_guards;
mod nonblocking;
mod redis_mro;
mod resolve;
mod thread_guards;
mod visit;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use async_doctor_core::{FunctionId, LiteralArg, ParamGuard, ProjectFacts};
use ruff_python_ast::{self as ast, Expr};
use ruff_text_size::TextRange;
use ty_python_semantic::SemanticModel;

use crate::sitepkgs::{FollowTarget, PackageMetaMemo};

use super::constants::ModuleConst;
use super::ctx::{DefKey, FileCtx, TaskTargets};

pub(crate) struct CallCollector<'a, 'db> {
    // Injected refs — set via [`CallCollector::new`]. Visit-state fields default
    // there so pass2/follow do not re-list every counter and HashSet.
    pub(super) ctx: &'a FileCtx<'db>,
    pub(super) model: &'a SemanticModel<'db>,
    pub(super) facts: &'a mut ProjectFacts,
    /// Optional shared base functions for parallel pass-2 so workers do not
    /// clone the full pass-1 function table per file.
    pub(super) functions_base: Option<&'a [async_doctor_core::FunctionFact]>,
    pub(super) def_map: &'a mut HashMap<DefKey, FunctionId>,
    /// Optional shared base for parallel pass-2: lookups fall through here so
    /// `def_map` only holds this file's inserts (no per-file full-map clone).
    pub(super) def_map_base: Option<&'a HashMap<DefKey, FunctionId>>,
    pub(super) qual_map: &'a mut HashMap<(String, String), FunctionId>,
    pub(super) qual_map_base: Option<&'a HashMap<(String, String), FunctionId>>,
    pub(super) property_defs: &'a HashSet<FunctionId>,
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
    pub(super) class_def_stack: Vec<&'a ast::StmtClassDef>,
    pub(super) fn_stack: Vec<String>,
    pub(super) dead_depth: u32,
    pub(super) loop_depth: u32,
    pub(super) branch_depth: u32,
    pub(super) cache_miss_depth: u32,
    /// Depth under a "no running event loop" guard (see [`ConditionTag::NoRunningLoopGuard`]).
    pub(super) no_running_loop_depth: u32,
    /// Depth under a proven off-loop-thread guard (`x != threading.get_ident()`, …).
    /// Blocking calls here do not starve the event loop — skip recording.
    pub(super) off_loop_thread_depth: u32,
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
    pub(super) pending_is_bound_call: bool,
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
    pub(super) pending_keyword_true: Vec<String>,
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
    /// Local name → stack of generator expressions (oldest → newest).
    /// Self-reassignment keeps prior bindings so
    /// `out = (sleep…); out = (x for x in out)` still expands the sleep body.
    pub(super) local_genexprs: HashMap<String, Vec<&'a ast::ExprGenerator>>,
    /// Genexprs currently being expanded while consuming (cycle guard for
    /// `out = (x for x in out)` then `tuple(out)`).
    pub(super) expanding_genexprs: HashSet<*const ast::ExprGenerator>,
    /// Nesting depth of genexpr expansions (hard cap beyond the pointer set).
    pub(super) genexpr_expand_depth: u32,
    /// Genexprs already expanded under the *current* top-level consume
    /// (cleared when `genexpr_expand_depth` returns to 0). Dedupes fan-out
    /// like `g = (a for a in prev for b in prev)` so expansion stays linear.
    pub(super) expanded_genexpr_memo: HashSet<*const ast::ExprGenerator>,
    /// Genexpr body visits under the current top-level consume (hard budget).
    pub(super) genexpr_expand_visits: u32,
    /// Locals bound only when no event loop is running (ternary / idle-loop helper).
    /// `if name is not None:` (or truthy `if name:`) is a [`ConditionTag::NoRunningLoopGuard`].
    pub(super) idle_loop_locals: HashSet<String>,
    /// Function/method names whose body returns a loop only when it is not running.
    pub(super) idle_loop_helpers: HashSet<String>,
    /// Names bound to an unbounded `queue.Queue()` / `Queue(maxsize<=0)`.
    /// `put` on these never blocks.
    pub(super) unbounded_queues: HashSet<String>,
    /// Names bound from `boto3.client` / `boto3.resource` (module or local).
    /// Dynamic client methods often have no stubs — track syntactically.
    pub(super) boto_receivers: HashSet<String>,
    /// Names bound from sync `boto3.Session()` — `.client` / `.resource` on these
    /// are factories even when stubs do not resolve the attribute.
    pub(super) boto_session_receivers: HashSet<String>,
    /// Names rooted in `aioboto3` / `aiobotocore` (Session, client, resource).
    /// Never treated as sync boto receivers for AWS-op synthesis.
    pub(super) aio_boto_receivers: HashSet<String>,
    /// Locals assigned from `asyncio.get_running_loop()` (incl. `= None` after
    /// `except RuntimeError`). Early-return `if name is not None` only guards
    /// when the name is in this set.
    pub(super) running_loop_locals: HashSet<String>,
    /// Locals assigned from `asyncio.get_event_loop()`. Early-return only when
    /// combined with `.is_running()` (not bare `is not None`).
    pub(super) event_loop_locals: HashSet<String>,
    /// Locals bound to `io.BytesIO` / `StringIO` (syntactic + typed).
    pub(super) buffer_locals: HashSet<String>,
    /// Archive handles opened on an in-memory buffer (`ZipFile(BytesIO())`, …).
    pub(super) in_memory_archive_receivers: HashSet<String>,
    /// Receivers proven `.done()` in the current branch (`fut`, `self.x`, …) —
    /// matching `.result()` is non-blocking.
    pub(super) future_done_receivers: HashSet<String>,
    /// Base functions marked as offload wrappers while visiting with `functions_base`.
    pub(super) base_offload_wrappers: HashSet<FunctionId>,
    /// Base functions that invoke a positional parameter while visiting with `functions_base`.
    pub(super) base_invokes_params: HashMap<FunctionId, Vec<u32>>,
    /// Base functions labeled as routes while visiting with `functions_base`.
    pub(super) base_route_ids: HashSet<FunctionId>,
    /// Base sync functions labeled as routes while visiting with `functions_base`.
    pub(super) base_sync_route_ids: HashSet<FunctionId>,
    /// Function scopes enclosing the current def (index-aligned with
    /// `fn_stack`): used to detect decorator factories `D(fn)` whose returned
    /// nested def forwards `fn` into an offload boundary.
    pub(super) fn_scopes: Vec<FnScope>,
    /// Local HTTP clients (`httpx.AsyncClient` / `aiohttp.ClientSession`) bound
    /// in the current function body: `name -> (client_kind, ctor_range, in_loop)`.
    pub(super) local_ephemeral_clients: HashMap<String, (&'static str, TextRange, bool)>,
    /// `global` / `nonlocal` names declared in the current function body.
    pub(super) nonlocal_or_global_names: HashSet<String>,
    /// Locals bound to stateful async DB sessions/connections in the current function.
    pub(super) local_stateful_sessions: HashSet<String>,
}

/// One enclosing `def` scope for nested-def offload-wrapper detection.
pub(super) struct FnScope {
    /// The enclosing function's id (None for defs outside the qual map).
    pub fid: Option<FunctionId>,
    /// Its parameter names — a nested def forwarding one of these is the
    /// decorator-factory shape `D(fn) -> wrapper`.
    pub params: Vec<String>,
    /// Names this function returns directly (`return wrapper`), including
    /// returns inside if/try/with bodies but not inside nested defs/classes.
    pub returned: std::collections::HashSet<String>,
}

/// Hard cap on nested local-genexpr expansions (belt alongside the pointer set).
pub(super) const MAX_GENEXPR_EXPAND_DEPTH: u32 = 64;
/// Hard cap on total genexpr expansions per file (fan-out safety).
pub(super) const MAX_GENEXPR_EXPAND_VISITS: u32 = 4096;

impl<'a, 'db> CallCollector<'a, 'db> {
    /// Build a collector with empty visit state. Callers override `current_fn` /
    /// `current_param_names` when following into a known function body.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        ctx: &'a FileCtx<'db>,
        model: &'a SemanticModel<'db>,
        facts: &'a mut ProjectFacts,
        def_map: &'a mut HashMap<DefKey, FunctionId>,
        def_map_base: Option<&'a HashMap<DefKey, FunctionId>>,
        qual_map: &'a mut HashMap<(String, String), FunctionId>,
        qual_map_base: Option<&'a HashMap<(String, String), FunctionId>>,
        property_defs: &'a HashSet<FunctionId>,
        task_targets: &'a mut TaskTargets,
        follow_targets: &'a mut HashMap<String, FollowTarget>,
        package_meta: &'a mut PackageMetaMemo,
        followable_extra_roots: &'a [PathBuf],
        record_unresolved_imports: bool,
        module_constants: &'a HashMap<String, ModuleConst>,
    ) -> Self {
        Self {
            ctx,
            model,
            facts,
            functions_base: None,
            def_map,
            def_map_base,
            qual_map,
            qual_map_base,
            property_defs,
            task_targets,
            follow_targets,
            package_meta,
            followable_extra_roots,
            record_unresolved_imports,
            module_constants,
            current_fn: None,
            current_param_names: Vec::new(),
            class_stack: Vec::new(),
            class_def_stack: Vec::new(),
            fn_stack: Vec::new(),
            dead_depth: 0,
            loop_depth: 0,
            branch_depth: 0,
            cache_miss_depth: 0,
            no_running_loop_depth: 0,
            off_loop_thread_depth: 0,
            try_saw_get_loop: false,
            active_param_guard: None,
            consuming_generator: false,
            in_lambda: false,
            skip_lambda_body: false,
            in_callee_position: false,
            pending_is_awaited: false,
            pending_is_bound_call: false,
            pending_with_enter: false,
            in_eager_offload_arg: false,
            async_with_receivers: HashMap::new(),
            redis_family_memo: HashMap::new(),
            occurrence_counter: HashMap::new(),
            pending_literals: Vec::new(),
            pending_keyword_true: Vec::new(),
            pending_callables: Vec::new(),
            pending_externals: Vec::new(),
            pending_constructed_receivers: Vec::new(),
            pending_constructed_receiver: None,
            pending_keyword_callables: Vec::new(),
            narrowed_locals: HashMap::new(),
            local_lambdas: HashMap::new(),
            local_genexprs: HashMap::new(),
            expanding_genexprs: HashSet::new(),
            genexpr_expand_depth: 0,
            expanded_genexpr_memo: HashSet::new(),
            genexpr_expand_visits: 0,
            idle_loop_locals: HashSet::new(),
            idle_loop_helpers: HashSet::new(),
            unbounded_queues: HashSet::new(),
            boto_receivers: HashSet::new(),
            boto_session_receivers: HashSet::new(),
            aio_boto_receivers: HashSet::new(),
            running_loop_locals: HashSet::new(),
            event_loop_locals: HashSet::new(),
            buffer_locals: HashSet::new(),
            in_memory_archive_receivers: HashSet::new(),
            future_done_receivers: HashSet::new(),
            base_offload_wrappers: HashSet::new(),
            base_invokes_params: HashMap::new(),
            base_route_ids: HashSet::new(),
            base_sync_route_ids: HashSet::new(),
            fn_scopes: Vec::new(),
            local_ephemeral_clients: HashMap::new(),
            nonlocal_or_global_names: HashSet::new(),
            local_stateful_sessions: HashSet::new(),
        }
    }

    pub(super) fn function(&self, id: FunctionId) -> &async_doctor_core::FunctionFact {
        if let Some(base) = self.functions_base {
            let base_len = base.len();
            let idx = id.index();
            if idx < base_len {
                return &base[idx];
            }
            return &self.facts.functions[idx - base_len];
        }
        self.facts.function(id)
    }

    pub(super) fn next_function_id(&self) -> FunctionId {
        let base_len = self.functions_base.map(|b| b.len()).unwrap_or(0);
        FunctionId((base_len + self.facts.functions.len()) as u32)
    }

    pub(super) fn mark_offload_wrapper(&mut self, id: FunctionId) {
        if let Some(base) = self.functions_base {
            let base_len = base.len();
            let idx = id.index();
            if idx < base_len {
                self.base_offload_wrappers.insert(id);
            } else {
                self.facts.functions[idx - base_len].is_offload_wrapper = true;
            }
        } else {
            self.facts.functions[id.index()].is_offload_wrapper = true;
        }
    }

    pub(super) fn record_param_invoke(&mut self, id: FunctionId, param_idx: u32) {
        if let Some(base) = self.functions_base {
            let base_len = base.len();
            let idx = id.index();
            if idx < base_len {
                let entry = self
                    .base_invokes_params
                    .entry(id)
                    .or_insert_with(|| base[idx].invokes_params.clone());
                if !entry.contains(&param_idx) {
                    entry.push(param_idx);
                }
            } else {
                let invokes = &mut self.facts.functions[idx - base_len].invokes_params;
                if !invokes.contains(&param_idx) {
                    invokes.push(param_idx);
                }
            }
        } else {
            let invokes = &mut self.facts.functions[id.index()].invokes_params;
            if !invokes.contains(&param_idx) {
                invokes.push(param_idx);
            }
        }
    }

    pub(super) fn mark_imperative_route(&mut self, id: FunctionId) {
        if let Some(base) = self.functions_base {
            let base_len = base.len();
            let idx = id.index();
            if idx < base_len {
                let f = &base[idx];
                if f.label == async_doctor_core::EntryLabel::Other {
                    self.base_route_ids.insert(id);
                }
                if !f.is_async {
                    self.base_sync_route_ids.insert(id);
                }
            } else {
                let f = &mut self.facts.functions[idx - base_len];
                if f.label == async_doctor_core::EntryLabel::Other {
                    f.label = async_doctor_core::EntryLabel::Route;
                }
                if !f.is_async {
                    f.is_sync_route = true;
                }
            }
        } else {
            let f = &mut self.facts.functions[id.index()];
            if f.label == async_doctor_core::EntryLabel::Other {
                f.label = async_doctor_core::EntryLabel::Route;
            }
            if !f.is_async {
                f.is_sync_route = true;
            }
        }
    }

    pub(super) fn def_get(&self, key: &DefKey) -> Option<FunctionId> {
        self.def_map
            .get(key)
            .copied()
            .or_else(|| self.def_map_base.and_then(|b| b.get(key).copied()))
    }

    /// When inside a nested def, return the enclosing function whose parameter
    /// `name` is — provided every scope between it and the current def returns
    /// the next def (`D(fn) -> wrapper`, `D(fn) -> outer -> wrapper`).
    pub(super) fn enclosing_param_scope(&self, name: &str) -> Option<FunctionId> {
        let n = self.fn_scopes.len();
        if n < 2 {
            return None;
        }
        for i in (0..n - 1).rev() {
            let scope = &self.fn_scopes[i];
            if !scope.params.iter().any(|p| p == name) {
                continue;
            }
            let returned_chain = ((i + 1)..n).all(|j| {
                self.fn_scopes[j - 1]
                    .returned
                    .contains(self.fn_stack[j].as_str())
            });
            if returned_chain {
                return scope.fid;
            }
        }
        None
    }

    /// Inside a nested, returned def: an enclosing function's param forwarded
    /// as the *callable* argument to this callee may make it an inferred
    /// offload wrapper (`D(fn) -> wrapper` calling `real_wrapper(fn)`).
    /// Offload wrappers take the callable first, so only arg 0 counts —
    /// `run_in_threadpool(handler, conn, exc)` forwards `conn` as data.
    /// Checked after merge via [`ProjectFacts::offload_wrapper_refs`].
    pub(super) fn note_forwarded_enclosing_param(
        &mut self,
        call: &'a ast::ExprCall,
        target: async_doctor_core::DecoratorRef,
    ) {
        if self.fn_scopes.len() < 2 {
            return;
        }
        let Some(Expr::Name(n)) = call.arguments.args.first() else {
            return;
        };
        if let Some(owner) = self.enclosing_param_scope(n.id.as_str()) {
            self.facts
                .offload_wrapper_refs
                .entry(owner)
                .or_default()
                .push(target);
        }
    }

    pub(super) fn qual_get(&self, key: &(String, String)) -> Option<FunctionId> {
        self.qual_map
            .get(key)
            .copied()
            .or_else(|| self.qual_map_base.and_then(|b| b.get(key).copied()))
    }
}
