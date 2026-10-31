//! Transitive AST-import dependency fingerprints for per-file cache hits.
//!
//! `deps_fp(F)` hashes sorted `{rel}\t{content_fp_or_?}` lines for every
//! first-party file in F's transitive import closure, plus synthetic absent
//! probes for unresolved import heads. Cross-module inference in the review
//! scenarios flows through imports, so content-hash changes in that closure
//! invalidate F without a per-file salsa reset.

mod fingerprint;
mod roots;
mod scrape;

#[cfg(test)]
mod scrape_tests;

pub use fingerprint::{
    build_content_fps_parallel, build_deps_and_fps_parallel, live_deps_fp, live_deps_fps_parallel,
};
pub use roots::build_module_to_rel;

/// Sentinel content fingerprint for a probed-but-missing first-party path.
pub const ABSENT_DEP_FP: &str = "?";

/// Catalog entry needed to build the direct-import graph.
#[derive(Debug, Clone)]
pub struct DepCatalogEntry {
    pub abs: std::path::PathBuf,
    pub rel: String,
    pub module: String,
}
