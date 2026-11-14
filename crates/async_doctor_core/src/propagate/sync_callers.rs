//! Shared sync-caller graph analysis and `.venv` async-twin resolution helpers.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

use crate::graph::{CalleeRef, FunctionId, ProjectFacts};
use crate::report::{Finding, SinkSiteSummary};

use super::{AsyncTwinHit, SinkTable};

/// Precomputed reachability and reverse-caller index for sync-caller checks.
pub(super) struct SyncCallerIndex {
    edges_by_caller: HashMap<FunctionId, Vec<usize>>,
    callers_of: HashMap<FunctionId, Vec<usize>>,
    sync_only_callers: HashSet<FunctionId>,
}

impl SyncCallerIndex {
    pub(super) fn new(facts: &ProjectFacts) -> Self {
        let edges_by_caller = build_edges_by_caller(facts);
        let callers_of = build_callee_index(facts);
        let async_reachable = functions_reachable_from_async_entries(facts, &edges_by_caller);
        let sync_only_callers = sync_non_async_reachable_callers(facts, &async_reachable);
        Self {
            edges_by_caller,
            callers_of,
            sync_only_callers,
        }
    }

    pub(super) fn edges_by_caller(&self) -> &HashMap<FunctionId, Vec<usize>> {
        &self.edges_by_caller
    }

    /// True when `target_id` has at least one sync-only project caller (used by
    /// `offload-has-async-twin` advice to avoid breaking sync callers).
    pub(super) fn has_sync_only_caller(
        &self,
        facts: &ProjectFacts,
        target_id: FunctionId,
        ignore_edge_idx: Option<usize>,
    ) -> bool {
        self.sync_caller_summary(facts, target_id, ignore_edge_idx)
            .0
            > 0
    }

    /// Count distinct sync-only project callers reaching `target_id` (directly or
    /// through intermediate sync project helpers) and return `(count, sample_loc)`.
    pub(super) fn sync_caller_summary(
        &self,
        facts: &ProjectFacts,
        target_id: FunctionId,
        ignore_edge_idx: Option<usize>,
    ) -> (u32, Option<String>) {
        let mut sync_sites: BTreeMap<FunctionId, (&str, u32)> = BTreeMap::new();
        let mut visited: HashSet<FunctionId> = HashSet::new();
        let mut queue: VecDeque<FunctionId> = VecDeque::new();
        visited.insert(target_id);
        queue.push_back(target_id);

        while let Some(cur) = queue.pop_front() {
            let Some(edge_idxs) = self.callers_of.get(&cur) else {
                continue;
            };
            for &idx in edge_idxs {
                if Some(idx) == ignore_edge_idx {
                    continue;
                }
                let edge = &facts.edges[idx];
                if edge.is_offload_wrapper {
                    continue;
                }
                let caller_id = edge.caller;
                if self.sync_only_callers.contains(&caller_id) {
                    let loc = (edge.file.as_str(), edge.line);
                    sync_sites
                        .entry(caller_id)
                        .and_modify(|prev| {
                            if loc < *prev {
                                *prev = loc;
                            }
                        })
                        .or_insert(loc);
                    continue;
                }
                if is_sync_project_helper(facts, caller_id) && visited.insert(caller_id) {
                    queue.push_back(caller_id);
                }
            }
        }

        let count = sync_sites.len() as u32;
        let sample = sync_sites
            .values()
            .min()
            .map(|(file, line)| format!("{file}:{line}"));
        (count, sample)
    }

    /// Summarize `sinkSites` and populate `syncCallerCount` / `sampleSyncCaller`
    /// for sites inside or reached through a sync project helper.
    pub(super) fn summarize_sink_sites(
        &self,
        facts: &ProjectFacts,
        findings: &[Finding],
    ) -> Vec<SinkSiteSummary> {
        let sites_with_hops = crate::sink_sites::summarize_with_project_hops(findings);
        if sites_with_hops.is_empty() {
            return Vec::new();
        }

        let mut edges_by_loc: HashMap<(&str, u32), Vec<usize>> = HashMap::new();
        for (idx, edge) in facts.edges.iter().enumerate() {
            if !edge.is_offload_wrapper {
                edges_by_loc
                    .entry((edge.file.as_str(), edge.line))
                    .or_default()
                    .push(idx);
            }
        }
        for idxs in edges_by_loc.values_mut() {
            idxs.sort_by(|&a, &b| {
                let ea = &facts.edges[a];
                let eb = &facts.edges[b];
                (ea.occurrence, ea.text.as_str(), a).cmp(&(eb.occurrence, eb.text.as_str(), b))
            });
        }

        sites_with_hops
            .into_iter()
            .map(|(mut site, deepest_hop)| {
                if let Some(helper_id) =
                    resolve_site_sync_helper(facts, &edges_by_loc, &site, deepest_hop.as_ref())
                {
                    let (count, sample) = self.sync_caller_summary(facts, helper_id, None);
                    site.sync_caller_count = count;
                    site.sample_sync_caller = sample;
                }
                site
            })
            .collect()
    }
}

fn is_sync_project_helper(facts: &ProjectFacts, fid: FunctionId) -> bool {
    let f = facts.function(fid);
    !f.is_async && !f.is_third_party
}

fn resolve_site_sync_helper(
    facts: &ProjectFacts,
    edges_by_loc: &HashMap<(&str, u32), Vec<usize>>,
    site: &SinkSiteSummary,
    deepest_hop: Option<&(String, u32)>,
) -> Option<FunctionId> {
    if let Some(idxs) = edges_by_loc.get(&(site.file.as_str(), site.line)) {
        for &idx in idxs {
            let caller = facts.edges[idx].caller;
            if is_sync_project_helper(facts, caller) {
                return Some(caller);
            }
        }
    }

    if site.choke_point_finding_count > 0
        && let Some(idxs) =
            edges_by_loc.get(&(site.choke_point_file.as_str(), site.choke_point_line))
    {
        for match_text in [true, false] {
            for &idx in idxs {
                let e = &facts.edges[idx];
                if match_text && e.text != site.choke_point_text {
                    continue;
                }
                if let CalleeRef::Project(fid) = e.callee
                    && is_sync_project_helper(facts, fid)
                {
                    return Some(fid);
                }
                if is_sync_project_helper(facts, e.caller) {
                    return Some(e.caller);
                }
            }
        }
    }

    if let Some((hop_file, hop_line)) = deepest_hop
        && let Some(idxs) = edges_by_loc.get(&(hop_file.as_str(), *hop_line))
    {
        for &idx in idxs {
            let e = &facts.edges[idx];
            if let CalleeRef::Project(fid) = e.callee
                && is_sync_project_helper(facts, fid)
            {
                return Some(fid);
            }
            if is_sync_project_helper(facts, e.caller) {
                return Some(e.caller);
            }
        }
    }

    None
}

/// True when `twin.symbol`, all `twin.needs`, and (when non-empty) at least one
/// `twin.any_needs` entry resolve in `facts.resolved_symbols`.
pub(super) fn twin_resolves(facts: &ProjectFacts, twin: &AsyncTwinHit) -> bool {
    if !facts.resolved_symbols.contains(twin.symbol) {
        return false;
    }
    if !twin
        .needs
        .iter()
        .all(|n| facts.resolved_symbols.contains(*n))
    {
        return false;
    }
    twin.any_needs.is_empty()
        || twin
            .any_needs
            .iter()
            .any(|n| facts.resolved_symbols.contains(*n))
}

/// Qualname to use for twin lookup: prefer `constructed_receiver` when it names
/// a known sync client (e.g. `openai.AzureOpenAI`), else the call qualname, else
/// the canonical `sink_qualname` from the sink table.
pub(super) fn twin_lookup_key(
    sinks: &dyn SinkTable,
    qualname: &str,
    constructed_receiver: Option<&str>,
) -> Option<String> {
    if let Some(recv) = constructed_receiver
        && sinks.async_twin(recv).is_some()
    {
        return Some(recv.to_string());
    }
    if sinks.async_twin(qualname).is_some() {
        return Some(qualname.to_string());
    }
    let hit = sinks.lookup(qualname)?;
    if sinks.async_twin(hit.sink_qualname).is_some() {
        return Some(hit.sink_qualname.to_string());
    }
    None
}

fn build_edges_by_caller(facts: &ProjectFacts) -> HashMap<FunctionId, Vec<usize>> {
    let mut index: HashMap<FunctionId, Vec<usize>> = HashMap::new();
    for (idx, edge) in facts.edges.iter().enumerate() {
        if edge.is_offload_wrapper {
            continue;
        }
        index.entry(edge.caller).or_default().push(idx);
    }
    index
}

fn build_callee_index(facts: &ProjectFacts) -> HashMap<FunctionId, Vec<usize>> {
    let mut index: HashMap<FunctionId, Vec<usize>> = HashMap::new();
    for (idx, edge) in facts.edges.iter().enumerate() {
        if edge.is_offload_wrapper {
            continue;
        }
        if let CalleeRef::Project(fid) = edge.callee {
            index.entry(fid).or_default().push(idx);
        }
        for fid in edge.positional_callables.iter().flatten() {
            index.entry(*fid).or_default().push(idx);
        }
        for (_, fid) in &edge.keyword_callables {
            index.entry(*fid).or_default().push(idx);
        }
    }
    index
}

fn functions_reachable_from_async_entries(
    facts: &ProjectFacts,
    edges_by_caller: &HashMap<FunctionId, Vec<usize>>,
) -> HashSet<FunctionId> {
    let mut reach = HashSet::new();
    let mut queue = VecDeque::new();
    for f in &facts.functions {
        if f.is_async && !f.is_sync_route && !f.is_third_party && reach.insert(f.id) {
            queue.push_back(f.id);
        }
    }
    while let Some(id) = queue.pop_front() {
        let Some(edges) = edges_by_caller.get(&id) else {
            continue;
        };
        for &eidx in edges {
            let CalleeRef::Project(callee) = facts.edges[eidx].callee else {
                continue;
            };
            if reach.insert(callee) {
                queue.push_back(callee);
            }
        }
    }
    reach
}

fn sync_non_async_reachable_callers(
    facts: &ProjectFacts,
    async_reachable: &HashSet<FunctionId>,
) -> HashSet<FunctionId> {
    let mut out = HashSet::new();
    for f in &facts.functions {
        if f.is_async || f.is_third_party {
            continue;
        }
        if !async_reachable.contains(&f.id) {
            out.insert(f.id);
        }
    }
    out
}
