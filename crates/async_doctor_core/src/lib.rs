//! Call graph, propagation, grading, and report types. This crate must not depend on ty/ruff.

mod graph;
mod id;
mod propagate;
mod report;

pub use graph::{
    CallEdge, CallKind, CalleeRef, ConditionTag, FunctionFact, FunctionId, OffloadKind,
    ProjectFacts,
};
pub use id::{FindingIdParts, finding_id, fix_group_id};
pub use propagate::{SinkHit, SinkTable, analyze};
pub use report::{
    Candidate, ChainStep, EXIT_ERROR, EXIT_FINDINGS, EXIT_OK, Entry, EntryLabel, Finding, Fix,
    Location, Report, SCHEMA_VERSION,
};
