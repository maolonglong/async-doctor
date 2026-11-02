//! Call graph, propagation, grading, and report types. This crate must not depend on ty/ruff.

mod environment;
mod graph;
mod id;
mod propagate;
mod report;
mod sink_sites;

pub use environment::{EnvironmentKind, ScanEnvironment};
pub use graph::{
    AnalyzeOptions, CallEdge, CallKind, CalleeRef, ConditionTag, DecoratorRef, FunctionFact,
    FunctionId, LiteralArg, OffloadKind, ParamGuard, ProjectFacts, UnresolvedImport,
    is_non_production_entry_path,
};
pub use id::{FindingIdParts, finding_id, fix_group_id};
pub use propagate::{
    AsyncTwinHit, SinkHit, SinkTable, TwinConfidence, analyze, analyze_with_options,
};
pub use report::{
    Advice, AdviceCategory, Candidate, ChainStep, EXIT_ERROR, EXIT_FINDINGS, EXIT_OK, Entry,
    EntryLabel, Finding, Fix, Location, Report, SCHEMA_VERSION, SinkSiteSummary, UnresolvedPattern,
    UnusedSuppression,
};
pub use sink_sites::{group_choke_point, primary_sink_step, summarize as summarize_sink_sites};
