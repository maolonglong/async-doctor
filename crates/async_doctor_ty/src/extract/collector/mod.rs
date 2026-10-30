//! Call-graph edge collection over a single Python file.

mod calls;
mod control_flow;
mod resolve;
mod visit;

use std::collections::{HashMap, HashSet};

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
    pub(super) qual_map: &'a mut HashMap<(String, String), FunctionId>,
    pub(super) property_defs: &'a HashSet<DefKey>,
    pub(super) task_targets: &'a mut TaskTargets,
    pub(super) follow_targets: &'a mut HashMap<String, FollowTarget>,
    pub(super) package_meta: &'a mut PackageMetaMemo,
    pub(super) module_constants: &'a HashMap<String, ModuleConst>,
    pub(super) current_fn: Option<FunctionId>,
    pub(super) current_param_names: Vec<String>,
    pub(super) class_stack: Vec<String>,
    pub(super) fn_stack: Vec<String>,
    pub(super) dead_depth: u32,
    pub(super) loop_depth: u32,
    pub(super) branch_depth: u32,
    pub(super) cache_miss_depth: u32,
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
    /// Locals bound by `async with <asyncio-redis> as name` → receiver type qual.
    pub(super) async_with_receivers: HashMap<String, String>,
    pub(super) occurrence_counter: HashMap<(FunctionId, String), u32>,
    /// Literals for the call currently being recorded (set by handle_call).
    pub(super) pending_literals: Vec<Option<LiteralArg>>,
    /// Positional callables for the call currently being recorded.
    pub(super) pending_callables: Vec<Option<FunctionId>>,
    /// Local name → lambda assigned in the current function.
    pub(super) local_lambdas: HashMap<String, &'a ast::ExprLambda>,
    /// Local name → generator expression assigned in the current function.
    pub(super) local_genexprs: HashMap<String, &'a ast::ExprGenerator>,
}
