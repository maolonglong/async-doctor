//! Encode live [`ProjectFacts`] into per-file cache blobs.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use async_doctor_core::{
    CallKind, CalleeRef, EntryLabel, FunctionFact, FunctionId, LiteralArg, ProjectFacts,
};

use crate::cache::{CACHE_SCHEMA_VERSION, TOOL_VERSION};

use super::{
    CachedFileEdge, CachedFileFacts, CachedFileFn, CachedFileUnresolvedImport, CachedProjectRef,
};
use crate::extract::ctx::DefKey;

fn label_str(l: EntryLabel) -> &'static str {
    match l {
        EntryLabel::Route => "route",
        EntryLabel::Lifecycle => "lifecycle",
        EntryLabel::Task => "task",
        EntryLabel::Other => "other",
    }
}

fn call_kind_to_str(k: CallKind) -> &'static str {
    match k {
        CallKind::Call => "Call",
        CallKind::WithEnter => "WithEnter",
        CallKind::WithExit => "WithExit",
        CallKind::ForIter => "ForIter",
        CallKind::PropertyGet => "PropertyGet",
        CallKind::GeneratorConsume => "GeneratorConsume",
    }
}

/// Precomputed indexes for encoding many files without O(files × facts) rebuilds.
pub struct EncodeIndexes<'a> {
    pub def_by_id: HashMap<u32, &'a DefKey>,
    pub fns_by_file: HashMap<&'a str, Vec<&'a FunctionFact>>,
    pub name_start_by_id: HashMap<u32, u32>,
    pub edges_by_file: HashMap<&'a str, Vec<usize>>,
}

impl<'a> EncodeIndexes<'a> {
    pub fn build(facts: &'a ProjectFacts, def_map: &'a HashMap<DefKey, FunctionId>) -> Self {
        let mut def_by_id: HashMap<u32, &DefKey> = HashMap::with_capacity(def_map.len());
        let mut name_start_by_id: HashMap<u32, u32> = HashMap::with_capacity(def_map.len());
        for (key, id) in def_map {
            def_by_id.insert(id.0, key);
            name_start_by_id.insert(id.0, key.name_start);
        }
        let mut fns_by_file: HashMap<&str, Vec<&FunctionFact>> = HashMap::new();
        for f in &facts.functions {
            if f.is_third_party {
                continue;
            }
            fns_by_file.entry(f.file.as_str()).or_default().push(f);
        }
        let mut edges_by_file: HashMap<&str, Vec<usize>> = HashMap::new();
        for (i, e) in facts.edges.iter().enumerate() {
            let Some(caller) = facts.functions.get(e.caller.index()) else {
                continue;
            };
            if caller.is_third_party {
                continue;
            }
            edges_by_file
                .entry(caller.file.as_str())
                .or_default()
                .push(i);
        }
        Self {
            def_by_id,
            fns_by_file,
            name_start_by_id,
            edges_by_file,
        }
    }
}

/// One miss file to encode and save.
#[derive(Debug, Clone)]
pub struct MissFileSpec {
    pub rel: String,
    pub content_fp: String,
    pub deps_fp: String,
    pub unresolved_tp: Vec<String>,
    pub stub_fingerprint: String,
}

/// Build a per-file cache blob from live facts using precomputed indexes.
#[allow(clippy::too_many_arguments)]
pub fn file_facts_from_project_with_indexes(
    facts: &ProjectFacts,
    indexes: &EncodeIndexes<'_>,
    rel: &str,
    content_fp: &str,
    venv_fp: &str,
    deps_fp: String,
    unresolved_third_party_modules: Vec<String>,
    follow_targets: &HashMap<String, crate::sitepkgs::FollowTarget>,
    task_targets: &HashSet<FunctionId>,
    property_defs: &HashSet<DefKey>,
    stub_fingerprint: &str,
) -> CachedFileFacts {
    let file_fns = indexes
        .fns_by_file
        .get(rel)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let id_set: HashSet<FunctionId> = file_fns.iter().map(|f| f.id).collect();

    let functions: Vec<CachedFileFn> = file_fns
        .iter()
        .filter_map(|f| {
            let name_start = indexes.name_start_by_id.get(&f.id.0).copied()?;
            Some(CachedFileFn {
                name_start,
                line: f.line,
                module: f.module.clone(),
                qualname: f.qualname.clone(),
                is_async: f.is_async,
                is_generator: f.is_generator,
                label: label_str(f.label).to_string(),
                is_sync_route: f.is_sync_route,
                param_names: f.param_names.clone(),
                invokes_params: f.invokes_params.clone(),
                is_offload_wrapper: f.is_offload_wrapper,
                async_twins: f.async_twins.clone(),
            })
        })
        .collect();

    let project_ref = |id: FunctionId| -> Option<CachedProjectRef> {
        let f = facts.functions.get(id.index())?;
        if f.is_third_party {
            return None;
        }
        let key = indexes.def_by_id.get(&id.0)?;
        Some(CachedProjectRef {
            module: f.module.clone(),
            qualname: f.qualname.clone(),
            file: key.file.clone(),
            name_start: key.name_start,
        })
    };

    let edge_idxs = indexes
        .edges_by_file
        .get(rel)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let edges: Vec<CachedFileEdge> = edge_idxs
        .iter()
        .filter_map(|&ei| {
            let e = facts.edges.get(ei)?;
            if !id_set.contains(&e.caller) {
                return None;
            }
            let caller_name_start = *indexes.name_start_by_id.get(&e.caller.0)?;
            let (
                callee_kind,
                callee_qualname,
                callee_module,
                callee_file,
                callee_name_start,
                offload_kind,
                param_invoke_index,
            ) = match &e.callee {
                CalleeRef::External { qualname } => (
                    "external".to_string(),
                    qualname.clone(),
                    None,
                    None,
                    None,
                    None,
                    None,
                ),
                CalleeRef::Offload(k) => (
                    "offload".to_string(),
                    k.as_str().to_string(),
                    None,
                    None,
                    None,
                    Some(k.as_str().to_string()),
                    None,
                ),
                CalleeRef::Unresolved { reason } => (
                    "unresolved".to_string(),
                    reason.clone(),
                    None,
                    None,
                    None,
                    None,
                    None,
                ),
                CalleeRef::ParamInvoke { index } => (
                    "param_invoke".to_string(),
                    format!("param:{index}"),
                    None,
                    None,
                    None,
                    None,
                    Some(*index),
                ),
                CalleeRef::Project(id) => {
                    let f = &facts.functions[id.index()];
                    if f.is_third_party {
                        (
                            "external".to_string(),
                            f.qualname.clone(),
                            None,
                            None,
                            None,
                            None,
                            None,
                        )
                    } else {
                        (
                            "project".to_string(),
                            f.qualname.clone(),
                            Some(f.module.clone()),
                            Some(f.file.clone()),
                            indexes.def_by_id.get(&id.0).map(|k| k.name_start),
                            None,
                            None,
                        )
                    }
                }
            };
            Some(CachedFileEdge {
                caller_name_start,
                callee_kind,
                callee_qualname,
                callee_module,
                callee_file,
                callee_name_start,
                offload_kind,
                param_invoke_index,
                is_offload_wrapper: e.is_offload_wrapper,
                kind: call_kind_to_str(e.kind).to_string(),
                file: e.file.clone(),
                line: e.line,
                text: e.text.clone(),
                conditions: e
                    .conditions
                    .iter()
                    .map(|c| c.as_str().to_string())
                    .collect(),
                occurrence: e.occurrence,
                positional_literals: e
                    .positional_literals
                    .iter()
                    .map(|o| {
                        o.map(|l| match l {
                            LiteralArg::True => "true".to_string(),
                            LiteralArg::False => "false".to_string(),
                            LiteralArg::None => "none".to_string(),
                        })
                    })
                    .collect(),
                positional_callable_refs: e
                    .positional_callables
                    .iter()
                    .map(|o| o.and_then(project_ref))
                    .collect(),
                positional_externals: e.positional_externals.clone(),
                positional_constructed_receivers: e.positional_constructed_receivers.clone(),
                keyword_callable_refs: e
                    .keyword_callables
                    .iter()
                    .filter_map(|(k, id)| project_ref(*id).map(|r| (k.clone(), r)))
                    .collect(),
                param_guard_index: e.param_guard.map(|g| g.index),
                param_guard_inverted: e.param_guard.map(|g| g.inverted),
                is_awaited: e.is_awaited,
                callee_is_coroutine: e.callee_is_coroutine,
                is_eager_offload_arg: e.is_eager_offload_arg,
                context_from_caller: e.context_from_caller.and_then(project_ref),
                constructed_receiver: e.constructed_receiver.clone(),
            })
        })
        .collect();

    let _ = (follow_targets, unresolved_third_party_modules);

    CachedFileFacts {
        schema_version: CACHE_SCHEMA_VERSION,
        tool_version: TOOL_VERSION.to_string(),
        rel: rel.to_string(),
        content_fingerprint: content_fp.to_string(),
        venv_fingerprint: venv_fp.to_string(),
        deps_fp,
        stub_fingerprint: stub_fingerprint.to_string(),
        functions,
        edges,
        unresolved_imports: facts
            .unresolved_imports
            .iter()
            .filter(|u| u.file == rel)
            .map(|u| CachedFileUnresolvedImport {
                line: u.line,
                module: u.module.clone(),
                text: u.text.clone(),
            })
            .collect(),
        // v15: project meta + live follow; omit bulky per-file copies.
        unresolved_third_party_modules: Vec::new(),
        follow_targets: BTreeMap::new(),
        task_target_name_starts: task_targets
            .iter()
            .filter_map(|id| {
                let f = facts.functions.get(id.index())?;
                if f.file != rel {
                    return None;
                }
                indexes.name_start_by_id.get(&id.0).copied()
            })
            .collect(),
        property_def_name_starts: property_defs
            .iter()
            .filter(|k| k.file == rel)
            .map(|k| k.name_start)
            .collect(),
    }
}
/// Encode and save miss files in parallel using shared read-only indexes.
#[allow(clippy::too_many_arguments)]
pub fn save_miss_file_facts_parallel(
    cache_root: &Path,
    project_root: &Path,
    facts: &ProjectFacts,
    def_map: &HashMap<DefKey, FunctionId>,
    follow_targets: &HashMap<String, crate::sitepkgs::FollowTarget>,
    task_targets: &HashSet<FunctionId>,
    property_defs: &HashSet<DefKey>,
    venv_fp: &str,
    miss_specs: &[MissFileSpec],
) {
    if miss_specs.is_empty() {
        return;
    }
    let indexes = Arc::new(EncodeIndexes::build(facts, def_map));
    use rayon::prelude::*;
    miss_specs.par_iter().for_each(|spec| {
        let cached = file_facts_from_project_with_indexes(
            facts,
            &indexes,
            &spec.rel,
            &spec.content_fp,
            venv_fp,
            spec.deps_fp.clone(),
            spec.unresolved_tp.clone(),
            follow_targets,
            task_targets,
            property_defs,
            &spec.stub_fingerprint,
        );
        let _ = super::save_file_facts(cache_root, project_root, &cached);
    });
}
