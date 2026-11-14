//! Isolated per-file Pass-2 delta results and deterministic merge into [`ProjectFacts`].

use std::collections::{BTreeMap, HashMap, HashSet};

use async_doctor_core::{
    CallEdge, CalleeRef, DecoratorRef, EntryLabel, FunctionFact, FunctionId, ProjectFacts,
    UnresolvedImport,
};

use crate::sitepkgs::FollowTarget;

use super::super::ctx::{DefKey, TaskTargets};

pub(crate) struct Pass2FileResult {
    pub new_functions: Vec<FunctionFact>,
    pub new_def_keys: Vec<(DefKey, usize)>,
    pub edges: Vec<CallEdge>,
    pub follow_targets: HashMap<String, FollowTarget>,
    pub task_target_ids: HashSet<FunctionId>,
    /// Existing (pre-pass2) functions labeled Route by imperative registration.
    pub route_ids: HashSet<FunctionId>,
    pub sync_route_ids: HashSet<FunctionId>,
    /// Base-function field patches applied during this file's visit.
    pub base_patches: Vec<BaseFnPatch>,
    /// Decorator / forwarded-param refs recorded during this file's visit.
    pub decorator_refs: BTreeMap<FunctionId, Vec<DecoratorRef>>,
    pub offload_wrapper_refs: BTreeMap<FunctionId, Vec<DecoratorRef>>,
    pub unresolved_imports: Vec<UnresolvedImport>,
    pub unresolved_third_party_modules: Vec<String>,
    pub resolved_third_party_modules: Vec<String>,
    /// When set, this file was skipped (panic / hard limit); merge marks incomplete.
    pub skip_reason: Option<String>,
}

/// Mutations `CallCollector` makes on pre-existing (Pass-1) functions.
#[derive(Debug, Clone)]
pub(crate) struct BaseFnPatch {
    pub id: FunctionId,
    pub is_offload_wrapper: bool,
    pub invokes_params: Vec<u32>,
}

impl Pass2FileResult {
    pub(crate) fn empty() -> Self {
        Self {
            new_functions: Vec::new(),
            new_def_keys: Vec::new(),
            edges: Vec::new(),
            follow_targets: HashMap::new(),
            task_target_ids: HashSet::new(),
            route_ids: HashSet::new(),
            sync_route_ids: HashSet::new(),
            decorator_refs: BTreeMap::new(),
            offload_wrapper_refs: BTreeMap::new(),
            base_patches: Vec::new(),
            unresolved_imports: Vec::new(),
            unresolved_third_party_modules: Vec::new(),
            resolved_third_party_modules: Vec::new(),
            skip_reason: None,
        }
    }

    pub(crate) fn skipped(rel: &str, reason: &str) -> Self {
        let mut r = Self::empty();
        r.skip_reason = Some(format!("{rel}: {reason}"));
        r
    }
}

/// Merge per-file Pass-2 results into `facts` in `results` order.
pub(crate) fn merge_pass2_results(
    facts: &mut ProjectFacts,
    def_map: &mut HashMap<DefKey, FunctionId>,
    qual_map: &mut HashMap<(String, String), FunctionId>,
    follow_targets: &mut HashMap<String, FollowTarget>,
    task_targets: &mut TaskTargets,
    base_fn_len: usize,
    results: Vec<Pass2FileResult>,
) {
    for result in results {
        if let Some(reason) = result.skip_reason {
            facts.complete = false;
            facts.skipped.push(reason);
            continue;
        }
        let mut remap: HashMap<FunctionId, FunctionId> = HashMap::new();
        for (i, mut f) in result.new_functions.into_iter().enumerate() {
            let provisional = FunctionId((base_fn_len + i) as u32);
            let new_id = FunctionId(facts.functions.len() as u32);
            remap.insert(provisional, new_id);
            f.id = new_id;
            qual_map.insert((f.module.clone(), f.qualname.clone()), new_id);
            facts.functions.push(f);
        }
        for (key, local_idx) in result.new_def_keys {
            let provisional = FunctionId((base_fn_len + local_idx) as u32);
            if let Some(&new_id) = remap.get(&provisional) {
                def_map.insert(key, new_id);
            }
        }
        for patch in result.base_patches {
            let id = remap.get(&patch.id).copied().unwrap_or(patch.id);
            if let Some(f) = facts.functions.get_mut(id.index()) {
                f.is_offload_wrapper = f.is_offload_wrapper || patch.is_offload_wrapper;
                for p in patch.invokes_params {
                    if !f.invokes_params.contains(&p) {
                        f.invokes_params.push(p);
                    }
                }
                f.invokes_params.sort_unstable();
            }
        }
        for (fid, refs) in result.decorator_refs {
            let fid = remap.get(&fid).copied().unwrap_or(fid);
            facts
                .decorator_refs
                .entry(fid)
                .or_default()
                .extend(refs.into_iter().map(|r| remap_ref(r, &remap)));
        }
        for (fid, refs) in result.offload_wrapper_refs {
            let fid = remap.get(&fid).copied().unwrap_or(fid);
            facts
                .offload_wrapper_refs
                .entry(fid)
                .or_default()
                .extend(refs.into_iter().map(|r| remap_ref(r, &remap)));
        }
        for mut edge in result.edges {
            remap_edge_ids(&mut edge, &remap);
            facts.edges.push(edge);
        }
        for (q, t) in result.follow_targets {
            follow_targets.entry(q).or_insert(t);
        }
        for id in result.task_target_ids {
            let id = remap.get(&id).copied().unwrap_or(id);
            task_targets.0.insert(id);
        }
        for id in result.route_ids {
            let id = remap.get(&id).copied().unwrap_or(id);
            let f = &mut facts.functions[id.index()];
            if f.label == EntryLabel::Other {
                f.label = EntryLabel::Route;
            }
        }
        for id in result.sync_route_ids {
            let id = remap.get(&id).copied().unwrap_or(id);
            facts.functions[id.index()].is_sync_route = true;
        }
        facts.unresolved_imports.extend(result.unresolved_imports);
        facts
            .unresolved_third_party_modules
            .extend(result.unresolved_third_party_modules);
        facts
            .resolved_third_party_modules
            .extend(result.resolved_third_party_modules);
    }
}

fn remap_ref(r: DecoratorRef, remap: &HashMap<FunctionId, FunctionId>) -> DecoratorRef {
    match r {
        DecoratorRef::Function(id) => DecoratorRef::Function(remap.get(&id).copied().unwrap_or(id)),
        other => other,
    }
}

fn remap_edge_ids(edge: &mut CallEdge, remap: &HashMap<FunctionId, FunctionId>) {
    if let Some(&id) = remap.get(&edge.caller) {
        edge.caller = id;
    }
    if let CalleeRef::Project(id) = &mut edge.callee
        && let Some(&nid) = remap.get(id)
    {
        *id = nid;
    }
    for slot in &mut edge.positional_callables {
        if let Some(id) = slot.as_mut()
            && let Some(&nid) = remap.get(id)
        {
            *id = nid;
        }
    }
    for (_, id) in &mut edge.keyword_callables {
        if let Some(&nid) = remap.get(id) {
            *id = nid;
        }
    }
    if let Some(id) = edge.context_from_caller.as_mut()
        && let Some(&nid) = remap.get(id)
    {
        *id = nid;
    }
}
