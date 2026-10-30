//! Call graph, propagation, grading, and report types. This crate must not depend on ty/ruff.

mod graph;
mod id;
mod propagate;
mod report;
mod sink_sites;

pub use graph::{
    AnalyzeOptions, CallEdge, CallKind, CalleeRef, ConditionTag, FunctionFact, FunctionId,
    LiteralArg, OffloadKind, ParamGuard, ProjectFacts, is_non_production_entry_path,
};
pub use id::{FindingIdParts, finding_id, fix_group_id};
pub use propagate::{SinkHit, SinkTable, analyze, analyze_with_options};
pub use report::{
    Candidate, ChainStep, EXIT_ERROR, EXIT_FINDINGS, EXIT_OK, Entry, EntryLabel, Finding, Fix,
    Location, Report, SCHEMA_VERSION, SinkSiteSummary, UnresolvedPattern, UnusedSuppression,
};
pub use sink_sites::{group_choke_point, summarize as summarize_sink_sites};
