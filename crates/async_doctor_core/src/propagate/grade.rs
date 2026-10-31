//! Grading helpers: conditions, param guards, file-IO merge, shortest paths.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

use crate::graph::{CalleeRef, ConditionTag, FunctionId, LiteralArg, ParamGuard, ProjectFacts};
use crate::report::{ChainStep, EntryLabel, Finding};

pub(super) fn label_rank(label: EntryLabel) -> u8 {
    match label {
        EntryLabel::Route => 0,
        EntryLabel::Lifecycle => 1,
        EntryLabel::Task => 2,
        EntryLabel::Other => 3,
    }
}

pub(super) fn chain_step_for(
    caller: &crate::graph::FunctionFact,
    file: String,
    line: u32,
    text: String,
    resolution: String,
) -> ChainStep {
    let text = truncate_hop_text(&text);
    let step = if caller.is_third_party {
        ChainStep::third_party(file, line, text, caller.package_name.clone())
    } else {
        ChainStep::project(file, line, text)
    };
    step.with_resolution(resolution)
}

/// Describe how a call-graph edge was resolved (for `why` evidence).
pub(super) fn resolution_for_edge(facts: &ProjectFacts, edge: &crate::graph::CallEdge) -> String {
    use crate::graph::CalleeRef;
    match &edge.callee {
        CalleeRef::Project(id) => {
            let callee = facts.function(*id);
            if callee.is_third_party {
                match &callee.package_name {
                    Some(pkg) => {
                        format!("third-party follow → `{}` ({pkg})", callee.full_qualname())
                    }
                    None => format!("third-party follow → `{}`", callee.full_qualname()),
                }
            } else {
                format!("resolved call → `{}`", callee.full_qualname())
            }
        }
        CalleeRef::External { qualname } => {
            format!("typed external → `{qualname}`")
        }
        CalleeRef::Unresolved { reason } => format!("unresolved ({reason})"),
        CalleeRef::Offload(kind) => format!("offload boundary (`{}`)", kind.as_str()),
        CalleeRef::ParamInvoke { index } => {
            format!("higher-order arg (positional {index})")
        }
    }
}

/// Chain hops must be a single line — multi-line call text (httpx/es wrappers)
/// collapses to the first non-empty line so agents see one actionable site.
pub(super) fn truncate_hop_text(text: &str) -> String {
    let line = text
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or(text.trim());
    line.to_string()
}

/// Aggregate key for unresolved sites: prefer a stable callee-ish snippet.
pub(super) fn unresolved_pattern(reason: &str, text: &str) -> String {
    let t = text.trim();
    // Keep a short head of the call text (before `(`) when present.
    if let Some(head) = t.split('(').next() {
        let head = head.trim();
        if !head.is_empty() && head.len() <= 80 {
            return format!("{reason}:{head}");
        }
    }
    reason.to_string()
}

/// Merge hop tags into a coherent list: never keep unconditional/all-paths
/// alongside any branch-like tag.
pub(super) fn reconcile_conditions(tags: &[ConditionTag]) -> Vec<String> {
    let has_branch = tags.iter().any(|t| t.is_branch_like());
    let has_loop = tags.contains(&ConditionTag::Loop);
    let has_cache = tags.contains(&ConditionTag::CacheMissDependent);
    let has_async_branch = tags.contains(&ConditionTag::AsyncBranch);
    let mut out = Vec::new();
    if has_loop {
        out.push(ConditionTag::Loop.as_str().to_string());
    }
    if has_cache {
        out.push(ConditionTag::CacheMissDependent.as_str().to_string());
    }
    // Emit async-branch for if/else/try/match (and when both cache + other branch tags appear).
    if has_async_branch || (has_branch && !has_cache) {
        out.push(ConditionTag::AsyncBranch.as_str().to_string());
    }
    if !has_branch && !has_loop {
        if tags.contains(&ConditionTag::Unconditional) {
            out.push(ConditionTag::Unconditional.as_str().to_string());
        }
        if tags.contains(&ConditionTag::AllPaths) {
            out.push(ConditionTag::AllPaths.as_str().to_string());
        }
        if out.is_empty() {
            out.push(ConditionTag::Unconditional.as_str().to_string());
            out.push(ConditionTag::AllPaths.as_str().to_string());
        }
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ParamVerdict {
    ProvablyTrue,
    ProvablyFalse,
    Unknown,
}

pub(super) fn evaluate_param_guards(
    facts: &ProjectFacts,
    path: &PathToEntry,
    guards: &[(FunctionId, ParamGuard)],
) -> ParamVerdict {
    if guards.is_empty() {
        return ParamVerdict::ProvablyTrue;
    }
    let mut any_unknown = false;
    for &(fn_id, guard) in guards {
        // Guards in the async entry's own body depend on external input — keep
        // as findings (tagged async-branch). Only demote for callee guards.
        if fn_id == path.entry {
            continue;
        }
        // Find the edge on the path that calls into the guarded callee.
        let call_edge = path.edges.iter().find_map(|&eidx| {
            let e = &facts.edges[eidx];
            match e.callee {
                CalleeRef::Project(id) if id == fn_id => Some(e),
                _ => None,
            }
        });
        let Some(edge) = call_edge else {
            // No caller on this path — cannot prove; demote.
            any_unknown = true;
            continue;
        };
        let lit = edge
            .positional_literals
            .get(guard.index as usize)
            .copied()
            .flatten();
        match (lit, guard.inverted) {
            (Some(LiteralArg::True), false) | (Some(LiteralArg::False), true) => {}
            (Some(LiteralArg::False), false)
            | (Some(LiteralArg::None), false)
            | (Some(LiteralArg::True), true) => {
                return ParamVerdict::ProvablyFalse;
            }
            _ => any_unknown = true,
        }
    }
    if any_unknown {
        ParamVerdict::Unknown
    } else {
        ParamVerdict::ProvablyTrue
    }
}

/// Merge `open(...)` + subsequent `f.read()`/`f.write()` findings for the same entry.
pub(super) fn merge_file_io_findings(findings: Vec<Finding>) -> Vec<Finding> {
    let mut opens: Vec<Finding> = Vec::new();
    let mut others: Vec<Finding> = Vec::new();
    for f in findings {
        let sink_text = f.chain.last().map(|c| c.text.as_str()).unwrap_or("");
        if is_open_call_text(sink_text)
            || f.fix.fix_group_id.contains("open") && f.rule == "blocking-file"
        {
            // Prefer matching by last chain hop looking like open(
            if is_open_call_text(sink_text) {
                opens.push(f);
                continue;
            }
        }
        if f.rule == "blocking-file" && is_file_rw_text(sink_text) {
            others.push(f);
            continue;
        }
        others.push(f);
    }

    // For each open finding, absorb same-entry file rw findings in the same file.
    let mut absorbed: HashSet<usize> = HashSet::new();
    for open in &mut opens {
        for (i, rw) in others.iter().enumerate() {
            if absorbed.contains(&i) {
                continue;
            }
            if rw.rule != "blocking-file"
                || !is_file_rw_text(rw.chain.last().map(|c| c.text.as_str()).unwrap_or(""))
            {
                continue;
            }
            if rw.entry.file != open.entry.file || rw.entry.name != open.entry.name {
                continue;
            }
            // Same entry: append rw hop if not already present.
            for step in &rw.chain {
                if !open
                    .chain
                    .iter()
                    .any(|s| s.file == step.file && s.line == step.line && s.text == step.text)
                {
                    open.chain.push(step.clone());
                }
            }
            absorbed.insert(i);
        }
    }

    let mut out = opens;
    for (i, f) in others.into_iter().enumerate() {
        if !absorbed.contains(&i) {
            out.push(f);
        }
    }
    out
}

pub(super) fn is_open_call_text(text: &str) -> bool {
    let t = text.trim();
    t.starts_with("open(") || t.contains(".open(")
}

pub(super) fn is_file_rw_text(text: &str) -> bool {
    let t = text.trim();
    t.contains(".read(")
        || t.contains(".readline(")
        || t.contains(".readlines(")
        || t.contains(".write(")
        || t.contains(".writelines(")
}

pub(super) struct PathToEntry {
    pub(super) entry: FunctionId,
    /// Edges from entry toward the seed caller (may be empty if seed is inside entry).
    pub(super) edges: Vec<usize>,
}

pub(super) fn shortest_paths_to_async_entries(
    facts: &ProjectFacts,
    reverse: &BTreeMap<FunctionId, Vec<usize>>,
    seed_caller: FunctionId,
) -> Vec<PathToEntry> {
    let seed_fn = facts.function(seed_caller);
    // Third-party async helpers (site-packages / dist-packages) are never entries.
    if seed_fn.is_async && !seed_fn.is_sync_route && !seed_fn.is_third_party {
        return vec![PathToEntry {
            entry: seed_caller,
            edges: vec![],
        }];
    }

    // State is (node, required predecessor caller for the next hop).
    // `required` comes from higher-order `context_from_caller` so kwargs/partial
    // bindings do not leak across call sites of the same apply() helper.
    type State = (FunctionId, Option<FunctionId>);
    // via[parent_state] = (child_state_toward_seed, edge_idx)
    let mut via: HashMap<State, (State, usize)> = HashMap::new();
    let mut visited: HashSet<State> = HashSet::new();
    let mut queue: VecDeque<State> = VecDeque::new();
    let seed_state: State = (seed_caller, None);
    queue.push_back(seed_state);
    visited.insert(seed_state);

    let mut entries = Vec::new();

    while let Some((node, required)) = queue.pop_front() {
        let Some(preds) = reverse.get(&node) else {
            continue;
        };
        for &edge_idx in preds {
            let edge = &facts.edges[edge_idx];
            if let Some(req) = required
                && edge.caller != req
            {
                continue;
            }
            let pred = edge.caller;
            let next_req = edge.context_from_caller;
            let pred_state: State = (pred, next_req);
            if !visited.insert(pred_state) {
                continue;
            }
            via.insert(pred_state, ((node, required), edge_idx));
            let pred_fn = facts.function(pred);
            // Never treat installed third-party async as an entry, and do not walk
            // *through* it toward first-party callers (stops FPs via awaited
            // library internals). Outside-editable follow-only async may be
            // traversed so scan-root callers still surface.
            if pred_fn.is_installed_third_party() && pred_fn.is_async {
                continue;
            }
            if pred_fn.is_async && !pred_fn.is_sync_route {
                let mut edges = Vec::new();
                let mut cur = pred_state;
                while cur.0 != seed_caller {
                    let Some(&(child, eidx)) = via.get(&cur) else {
                        break;
                    };
                    edges.push(eidx);
                    cur = child;
                }
                entries.push(PathToEntry { entry: pred, edges });
            } else {
                queue.push_back(pred_state);
            }
        }
    }

    entries
}

pub(super) fn async_twin_hint(
    facts: &ProjectFacts,
    entry_id: FunctionId,
    path: &PathToEntry,
) -> Option<String> {
    let host = if let Some(&last) = path.edges.last() {
        match facts.edges[last].callee {
            CalleeRef::Project(fid) => facts.function(fid),
            _ => facts.function(entry_id),
        }
    } else {
        facts.function(entry_id)
    };

    let method = host.qualname.rsplit('.').next().unwrap_or(&host.qualname);
    for (sync_name, twin) in &host.async_twins {
        if sync_name == method {
            let class = host.qualname.rsplit_once('.').map(|(c, _)| c).unwrap_or("");
            let q = if class.is_empty() {
                twin.clone()
            } else {
                format!("{class}.{twin}")
            };
            return Some(q);
        }
    }
    None
}
