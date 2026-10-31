//! Per-file first-party fact cache for incremental warm extracts.
//!
//! Layout: `{cache_root}/v{schema}/{tool}/files/{project_hash}/{rel_hash}.json.gz`
//! plus `_fp_index.json.gz` (content/deps/venv/stub fingerprints only).
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

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use serde::{Deserialize, Serialize};

use crate::cache::{CACHE_SCHEMA_VERSION, CachedFollowTarget, TOOL_VERSION, sanitize_component};

pub use deps::{
    DepCatalogEntry, build_content_fps_parallel, build_deps_and_fps_parallel, build_module_to_rel,
    live_deps_fp, live_deps_fps_parallel,
};
pub use encode::{MissFileSpec, save_miss_file_facts_parallel};
pub use graph::{content_venv_stub_match, project_env_fingerprints, wide_miss_fallback};
pub use merge::{append_cached_edges_only, cached_project_callees_resolvable, merge_cached_files};

fn is_false(v: &bool) -> bool {
    !*v
}

/// One first-party function body, keyed for remapping without global ids.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CachedFileFn {
    pub name_start: u32,
    pub line: u32,
    pub module: String,
    pub qualname: String,
    #[serde(default, skip_serializing_if = "is_false")]
    pub is_async: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub is_generator: bool,
    pub label: String,
    #[serde(default, skip_serializing_if = "is_false")]
    pub is_sync_route: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_names: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub invokes_params: Vec<u32>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub is_offload_wrapper: bool,
    /// `(sync_method, async_twin)` pairs — stored explicitly so merge does not
    /// re-derive twins from incomplete class-method sets.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
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
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub file: String,
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub name_start: u32,
}

fn is_zero_u32(v: &u32) -> bool {
    *v == 0
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CachedFileEdge {
    pub caller_name_start: u32,
    pub callee_kind: String,
    pub callee_qualname: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub callee_module: Option<String>,
    /// First-party Project callee file (relative); empty for non-project.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub callee_file: Option<String>,
    /// First-party Project callee `name_start` for DefKey remapping.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub callee_name_start: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offload_kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub param_invoke_index: Option<u32>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub is_offload_wrapper: bool,
    pub kind: String,
    /// Empty means the slot's `rel` (omitted on save when equal).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub file: String,
    pub line: u32,
    pub text: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conditions: Vec<String>,
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub occurrence: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub positional_literals: Vec<Option<String>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub positional_callable_refs: Vec<Option<CachedProjectRef>>,
    /// Positional External/stdlib quals (offload targets for advice rules).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub positional_externals: Vec<Option<String>>,
    /// Positional constructed client class roots (AzureOpenAI twin preference).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub positional_constructed_receivers: Vec<Option<String>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keyword_callable_refs: Vec<(String, CachedProjectRef)>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub param_guard_index: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub param_guard_inverted: Option<bool>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub is_awaited: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub callee_is_coroutine: Option<bool>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub is_eager_offload_arg: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_from_caller: Option<CachedProjectRef>,
    /// Attribute-callee constructed client class root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub functions: Vec<CachedFileFn>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub edges: Vec<CachedFileEdge>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unresolved_imports: Vec<CachedFileUnresolvedImport>,
    /// Deprecated in v15 (kept for serde default); live follow rediscovers.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unresolved_third_party_modules: Vec<String>,
    /// Deprecated in v15; empty on save. Seeded follow still works when present.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub follow_targets: BTreeMap<String, CachedFollowTarget>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub task_target_name_starts: Vec<u32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub property_def_name_starts: Vec<u32>,
}

/// Fingerprints only — used to classify hits without gunzipping full slots.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SlotFpEntry {
    pub content_fingerprint: String,
    pub deps_fp: String,
    #[serde(default)]
    pub venv_fingerprint: String,
    #[serde(default)]
    pub stub_fingerprint: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct FpIndex {
    pub schema_version: u32,
    pub tool_version: String,
    pub entries: HashMap<String, SlotFpEntry>,
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
    // v15+: gzip-compressed JSON (`.json.gz`).
    project_files_dir(root, project_root).join(format!("{}.json.gz", sanitize_component(&rel_hash)))
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
    let raw = fs::read(path).ok()?;
    let mut dec = GzDecoder::new(raw.as_slice());
    let mut data = String::new();
    dec.read_to_string(&mut data).ok()?;
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
    let data = serde_json::to_vec(cached)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    let tmp = path.with_extension(format!("gz.tmp.{}", std::process::id()));
    {
        let f = fs::File::create(&tmp)?;
        let mut enc = GzEncoder::new(f, Compression::default());
        enc.write_all(&data)?;
        enc.finish()?;
    }
    fs::rename(&tmp, &path)?;
    Ok(())
}

fn fp_index_path(root: &Path, project_root: &Path) -> PathBuf {
    project_files_dir(root, project_root).join("_fp_index.json.gz")
}

/// Load the project fingerprint index, if present and schema-compatible.
pub fn load_fp_index(root: &Path, project_root: &Path) -> Option<FpIndex> {
    let path = fp_index_path(root, project_root);
    let raw = fs::read(path).ok()?;
    let mut dec = GzDecoder::new(raw.as_slice());
    let mut data = String::new();
    dec.read_to_string(&mut data).ok()?;
    let index: FpIndex = serde_json::from_str(&data).ok()?;
    if index.schema_version != CACHE_SCHEMA_VERSION || index.tool_version != TOOL_VERSION {
        return None;
    }
    Some(index)
}

/// Atomically write the fingerprint index.
pub fn save_fp_index(root: &Path, project_root: &Path, index: &FpIndex) -> std::io::Result<()> {
    let path = fp_index_path(root, project_root);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let data = serde_json::to_vec(index)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    let tmp = path.with_extension(format!("gz.tmp.{}", std::process::id()));
    {
        let f = fs::File::create(&tmp)?;
        let mut enc = GzEncoder::new(f, Compression::default());
        enc.write_all(&data)?;
        enc.finish()?;
    }
    fs::rename(&tmp, &path)?;
    Ok(())
}

/// Upsert fingerprint entries for freshly saved miss slots.
pub fn upsert_fp_index_for_misses(
    root: &Path,
    project_root: &Path,
    venv_fp: &str,
    miss_specs: &[MissFileSpec],
) {
    if miss_specs.is_empty() {
        return;
    }
    let mut index = load_fp_index(root, project_root).unwrap_or_else(|| FpIndex {
        schema_version: CACHE_SCHEMA_VERSION,
        tool_version: TOOL_VERSION.to_string(),
        entries: HashMap::new(),
    });
    index.schema_version = CACHE_SCHEMA_VERSION;
    index.tool_version = TOOL_VERSION.to_string();
    for spec in miss_specs {
        index.entries.insert(
            spec.rel.clone(),
            SlotFpEntry {
                content_fingerprint: spec.content_fp.clone(),
                deps_fp: spec.deps_fp.clone(),
                venv_fingerprint: venv_fp.to_string(),
                stub_fingerprint: spec.stub_fingerprint.clone(),
            },
        );
    }
    let _ = save_fp_index(root, project_root, &index);
}

/// Content/venv/stub match against an index entry (no deps_fp).
pub fn index_content_venv_stub_match(
    entry: &SlotFpEntry,
    content_fp: &str,
    venv_fp: &str,
    stub_fp: &str,
) -> bool {
    entry.content_fingerprint == content_fp
        && entry.venv_fingerprint == venv_fp
        && (entry.stub_fingerprint.is_empty() || entry.stub_fingerprint == stub_fp)
}
pub(super) fn short_hash(bytes: &[u8]) -> String {
    let mut hash: u64 = 0xcbf29ce484222325;
    for b in bytes {
        hash ^= u64::from(*b);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}
