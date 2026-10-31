//! Per-file first-party fact cache for incremental warm extracts.
//!
//! Layout: `{cache_root}/v{schema}/{tool}/files/{project_hash}/{rel_hash}.json`
//!
//! FunctionIds are not stored — callees use `(module, qualname)`. Merge assigns
//! stable ids by sorted file order so cold and incremental-warm agree.
//!
//! Invalidation is **sound by construction**: each slot stores a transitive
//! AST-import dependency fingerprint (`deps_fp`). A hit requires the file's
//! own content/venv/stub fingerprints plus a live-recomputed `deps_fp` match.
//! See `docs/incremental_cache.md`.

mod deps;
mod encode;
mod graph;
mod merge;
pub(crate) mod orchestrate;
mod pass2_run;

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::cache::{CACHE_SCHEMA_VERSION, CachedFollowTarget, TOOL_VERSION, sanitize_component};

pub use deps::{
    DepCatalogEntry, build_deps_and_fps_parallel, build_module_to_rel, live_deps_fp,
    live_deps_fps_parallel,
};
pub use encode::{MissFileSpec, save_miss_file_facts_parallel};
pub use graph::{content_venv_stub_match, project_env_fingerprints, wide_miss_fallback};
pub use merge::{append_cached_edges_only, cached_project_callees_resolvable, merge_cached_files};

/// One first-party function body, keyed for remapping without global ids.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CachedFileFn {
    pub name_start: u32,
    pub line: u32,
    pub module: String,
    pub qualname: String,
    pub is_async: bool,
    pub is_generator: bool,
    pub label: String,
    pub is_sync_route: bool,
    pub param_names: Vec<String>,
    pub invokes_params: Vec<u32>,
    pub is_offload_wrapper: bool,
    /// `(sync_method, async_twin)` pairs — stored explicitly so merge does not
    /// re-derive twins from incomplete class-method sets.
    #[serde(default)]
    pub async_twins: Vec<(String, String)>,
}

/// Project callee / callable ref without FunctionId.
///
/// `file` + `name_start` disambiguate overloaded/nested same-qualname defs;
/// `module`/`qualname` remain for debugging and legacy slots.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CachedProjectRef {
    pub module: String,
    pub qualname: String,
    #[serde(default)]
    pub file: String,
    #[serde(default)]
    pub name_start: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CachedFileEdge {
    pub caller_name_start: u32,
    pub callee_kind: String,
    pub callee_qualname: String,
    pub callee_module: Option<String>,
    /// First-party Project callee file (relative); empty for non-project.
    #[serde(default)]
    pub callee_file: Option<String>,
    /// First-party Project callee `name_start` for DefKey remapping.
    #[serde(default)]
    pub callee_name_start: Option<u32>,
    pub offload_kind: Option<String>,
    pub param_invoke_index: Option<u32>,
    pub is_offload_wrapper: bool,
    pub kind: String,
    pub file: String,
    pub line: u32,
    pub text: String,
    pub conditions: Vec<String>,
    pub occurrence: u32,
    pub positional_literals: Vec<Option<String>>,
    pub positional_callable_refs: Vec<Option<CachedProjectRef>>,
    /// Positional External/stdlib quals (offload targets for advice rules).
    #[serde(default)]
    pub positional_externals: Vec<Option<String>>,
    /// Positional constructed client class roots (AzureOpenAI twin preference).
    #[serde(default)]
    pub positional_constructed_receivers: Vec<Option<String>>,
    pub keyword_callable_refs: Vec<(String, CachedProjectRef)>,
    pub param_guard_index: Option<u32>,
    pub param_guard_inverted: Option<bool>,
    pub is_awaited: bool,
    pub callee_is_coroutine: Option<bool>,
    pub is_eager_offload_arg: bool,
    pub context_from_caller: Option<CachedProjectRef>,
    /// Attribute-callee constructed client class root.
    #[serde(default)]
    pub constructed_receiver: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CachedFileUnresolvedImport {
    pub line: u32,
    pub module: String,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CachedFileFacts {
    pub schema_version: u32,
    pub tool_version: String,
    pub rel: String,
    pub content_fingerprint: String,
    /// Active venv dist-info fingerprint at save time.
    #[serde(default)]
    pub venv_fingerprint: String,
    /// Transitive AST-import dependency fingerprint (see `deps`).
    #[serde(default)]
    pub deps_fp: String,
    /// Project-wide first-party `.pyi` set fingerprint at save time.
    #[serde(default)]
    pub stub_fingerprint: String,
    pub functions: Vec<CachedFileFn>,
    pub edges: Vec<CachedFileEdge>,
    pub unresolved_imports: Vec<CachedFileUnresolvedImport>,
    pub unresolved_third_party_modules: Vec<String>,
    pub follow_targets: BTreeMap<String, CachedFollowTarget>,
    pub task_target_name_starts: Vec<u32>,
    pub property_def_name_starts: Vec<u32>,
}

/// Project-scoped sidecar (unresolved third-party heads are not per-file).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CachedProjectMeta {
    pub schema_version: u32,
    pub tool_version: String,
    pub venv_fingerprint: String,
    /// First-party `.pyi` set fingerprint (any stub change invalidates file slots).
    #[serde(default)]
    pub stub_fingerprint: String,
    pub unresolved_third_party_modules: Vec<String>,
}

fn files_dir(root: &Path) -> PathBuf {
    root.join(format!("v{CACHE_SCHEMA_VERSION}"))
        .join(sanitize_component(TOOL_VERSION))
        .join("files")
}

/// Scope per-file slots by project root so identical relative paths in different
/// trees (fixtures / temp clones) cannot collide.
fn project_files_dir(root: &Path, project_root: &Path) -> PathBuf {
    let key = short_hash(
        project_root
            .canonicalize()
            .unwrap_or_else(|_| project_root.to_path_buf())
            .to_string_lossy()
            .as_bytes(),
    );
    files_dir(root).join(sanitize_component(&key))
}

fn file_cache_path(root: &Path, project_root: &Path, rel: &str) -> PathBuf {
    let rel_hash = short_hash(rel.as_bytes());
    project_files_dir(root, project_root).join(format!("{}.json", sanitize_component(&rel_hash)))
}

fn project_meta_path(root: &Path, project_root: &Path) -> PathBuf {
    project_files_dir(root, project_root).join("_project_meta.json")
}

/// Load project-level meta (unresolved third-party heads + venv/stub echo).
pub fn load_project_meta(root: &Path, project_root: &Path) -> Option<CachedProjectMeta> {
    let data = fs::read_to_string(project_meta_path(root, project_root)).ok()?;
    let meta: CachedProjectMeta = serde_json::from_str(&data).ok()?;
    if meta.schema_version != CACHE_SCHEMA_VERSION || meta.tool_version != TOOL_VERSION {
        return None;
    }
    Some(meta)
}

pub fn save_project_meta(
    root: &Path,
    project_root: &Path,
    meta: &CachedProjectMeta,
) -> std::io::Result<()> {
    let path = project_meta_path(root, project_root);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let data = serde_json::to_string(meta)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    let tmp = path.with_extension(format!("json.tmp.{}", std::process::id()));
    {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(data.as_bytes())?;
    }
    fs::rename(&tmp, &path)?;
    Ok(())
}

/// FNV-1a of file bytes (stable; independent of mtime).
pub fn file_content_fingerprint(path: &Path) -> Option<String> {
    let bytes = fs::read(path).ok()?;
    Some(short_hash(&bytes))
}

/// Load the slot for `rel` without a content check (caller validates / compares).
pub fn load_file_facts_unchecked(
    root: &Path,
    project_root: &Path,
    rel: &str,
) -> Option<CachedFileFacts> {
    let path = file_cache_path(root, project_root, rel);
    let data = fs::read_to_string(path).ok()?;
    let cached: CachedFileFacts = serde_json::from_str(&data).ok()?;
    if cached.schema_version != CACHE_SCHEMA_VERSION || cached.tool_version != TOOL_VERSION {
        return None;
    }
    if cached.rel != rel {
        return None;
    }
    Some(cached)
}

/// True when a per-file cache slot exists for `rel` (may still be stale).
pub fn file_cache_slot_exists(root: &Path, project_root: &Path, rel: &str) -> bool {
    file_cache_path(root, project_root, rel).is_file()
}

pub fn save_file_facts(
    root: &Path,
    project_root: &Path,
    cached: &CachedFileFacts,
) -> std::io::Result<()> {
    let path = file_cache_path(root, project_root, &cached.rel);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let data = serde_json::to_string(cached)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    let tmp = path.with_extension(format!("json.tmp.{}", std::process::id()));
    {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(data.as_bytes())?;
    }
    fs::rename(&tmp, &path)?;
    Ok(())
}

pub(super) fn short_hash(bytes: &[u8]) -> String {
    let mut hash: u64 = 0xcbf29ce484222325;
    for b in bytes {
        hash ^= u64::from(*b);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}
