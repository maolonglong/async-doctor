//! Call-graph edge collection over a single Python file.

mod calls;
mod control_flow;
mod resolve;
mod visit;

use std::collections::{HashMap, HashSet};

use async_doctor_core::{FunctionId, LiteralArg, ParamGuard, ProjectFacts};
use ruff_python_ast::{self as ast};
use ty_python_semantic::SemanticModel;

use crate::sitepkgs::FollowTarget;

use super::constants::ModuleConst;
use super::ctx::{DefKey, FileCtx, TaskTargets};

pub(crate) struct CallCollector<'a, 'db> {
    pub(crate) ctx: &'a FileCtx<'db>,
    pub(crate) model: &'a SemanticModel<'db>,
    pub(crate) facts: &'a mut ProjectFacts,
    pub(crate) def_map: &'a mut HashMap<DefKey, FunctionId>,
    pub(crate) qual_map: &'a mut HashMap<(String, String), FunctionId>,
    pub(crate) property_defs: &'a HashSet<DefKey>,
    pub(crate) task_targets: &'a mut TaskTargets,
    pub(crate) follow_targets: &'a mut HashMap<String, FollowTarget>,
    pub(crate) module_constants: &'a HashMap<String, ModuleConst>,
    pub(crate) current_fn: Option<FunctionId>,
    pub(crate) current_param_names: Vec<String>,
    pub(crate) class_stack: Vec<String>,
    pub(crate) fn_stack: Vec<String>,
    pub(crate) dead_depth: u32,
    pub(crate) loop_depth: u32,
    pub(crate) branch_depth: u32,
    pub(crate) cache_miss_depth: u32,
    pub(crate) active_param_guard: Option<ParamGuard>,
    /// True while visiting an expression that consumes a generator.
    pub(crate) consuming_generator: bool,
    pub(crate) in_lambda: bool,
    /// When true, the enclosing lambda was passed to an offload boundary.
    pub(crate) skip_lambda_body: bool,
    /// True while visiting `call.func` — suppress PropertyGet on that Attribute.
    pub(crate) in_callee_position: bool,
    /// When true, the *next* recorded Call edge is the direct operand of
    /// `await` / `async for` iter / `async with` context (nested arg calls are not).
    pub(crate) pending_is_awaited: bool,
    /// Locals bound by `async with <asyncio-redis> as name` → receiver type qual.
    pub(crate) async_with_receivers: HashMap<String, String>,
    pub(crate) occurrence_counter: HashMap<(FunctionId, String), u32>,
    /// Literals for the call currently being recorded (set by handle_call).
    pub(crate) pending_literals: Vec<Option<LiteralArg>>,
    /// Positional callables for the call currently being recorded.
    pub(crate) pending_callables: Vec<Option<FunctionId>>,
    /// Local name → lambda assigned in the current function.
    pub(crate) local_lambdas: HashMap<String, &'a ast::ExprLambda>,
    /// Local name → generator expression assigned in the current function.
    pub(crate) local_genexprs: HashMap<String, &'a ast::ExprGenerator>,
}
