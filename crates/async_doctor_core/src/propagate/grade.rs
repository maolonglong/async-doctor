//! Grading helpers: conditions, param guards, file-IO merge, shortest paths.

mod merge;

pub(super) use merge::{merge_file_io_findings, merge_same_line_ctor_call_findings};

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

pub(super) type ItemSpanMap = BTreeMap<String, Vec<(String, u32, u32)>>;
pub(super) type ClassInitIndex = HashMap<(String, String), Vec<(String, u32, u32)>>;

use crate::graph::{
    CallEdge, CalleeRef, ConditionTag, DecoratorRef, FunctionId, LiteralArg, ParamGuard,
    ProjectFacts, is_non_production_entry_path,
};
use crate::id::{FindingIdParts, finding_id};
use crate::report::{ChainStep, EntryLabel};

/// Compute a collision-free finding/candidate/advice ID for a concrete site.
///
/// Re-visits of the exact same `(entry, file, line)` site return `None`
/// (deduplicated). Distinct sites whose raw `occurrence` collided because two
/// callees normalized to one canonical sink increment `occurrence` instead of
/// panicking or dropping the second site.
pub(super) fn unique_site_finding_id(
    mut parts: FindingIdParts<'_>,
    entry_id: FunctionId,
    site: (&str, u32),
    emitted_sites: &mut HashMap<String, (FunctionId, String, u32)>,
    emitted: &mut HashSet<String>,
) -> Option<String> {
    loop {
        let id = finding_id(parts);
        if let Some((prev_entry, prev_file, prev_line)) = emitted_sites.get(&id) {
            if *prev_entry == entry_id && prev_file == site.0 && *prev_line == site.1 {
                return None;
            }
            parts.occurrence = parts.occurrence.saturating_add(1);
            continue;
        }
        if emitted.contains(&id) {
            parts.occurrence = parts.occurrence.saturating_add(1);
            continue;
        }
        emitted_sites.insert(id.clone(), (entry_id, site.0.to_string(), site.1));
        emitted.insert(id.clone());
        return Some(id);
    }
}

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
        let param_name = facts
            .function(fn_id)
            .param_names
            .get(guard.index as usize)
            .map(String::as_str);
        let kw_binding = param_name.and_then(|name| {
            edge.keyword_literals
                .iter()
                .find(|(k, _)| k == name)
                .map(|(_, lit)| *lit)
        });
        let lit = if let Some(kw_lit) = kw_binding {
            kw_lit
        } else {
            let arg_idx = if edge.is_bound_call {
                guard.index.checked_sub(1)
            } else {
                Some(guard.index)
            };
            let Some(arg_idx) = arg_idx else {
                any_unknown = true;
                continue;
            };
            edge.positional_literals
                .get(arg_idx as usize)
                .copied()
                .flatten()
        };
        match (lit, guard.inverted) {
            (Some(LiteralArg::True), false)
            | (Some(LiteralArg::False), true)
            | (Some(LiteralArg::None), true) => {}
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

pub(super) struct PathToEntry {
    pub(super) entry: FunctionId,
    /// Edges from entry toward the seed caller (may be empty if seed is inside entry).
    pub(super) edges: Vec<usize>,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum RequiredPred {
    Any,
    Edge(usize),
    Caller(FunctionId),
}

pub(super) fn shortest_paths_to_async_entries(
    facts: &ProjectFacts,
    reverse: &[Vec<usize>],
    seed_caller: FunctionId,
) -> Vec<PathToEntry> {
    let mut entries = Vec::new();
    let seed_fn = facts.function(seed_caller);
    // Third-party async helpers (site-packages / dist-packages) are never entries.
    if seed_fn.is_async && !seed_fn.is_sync_route && !seed_fn.is_third_party {
        entries.push(PathToEntry {
            entry: seed_caller,
            edges: vec![],
        });
        // Production async defs stop here; non-production async helpers keep
        // walking upward so production routes awaiting them still surface.
        if !is_non_production_entry_path(&seed_fn.file) {
            return entries;
        }
    }

    // State is (node, required predecessor edge/caller for the next hop).
    // `required` comes from higher-order `context_from_edge` / `context_from_caller`
    // so callbacks bind to the exact call site that passed them.
    type State = (FunctionId, RequiredPred);
    // via[parent_state] = (child_state_toward_seed, edge_idx)
    let mut via: HashMap<State, (State, usize)> = HashMap::new();
    let mut visited: HashSet<State> = HashSet::new();
    let mut queue: VecDeque<State> = VecDeque::new();
    let seed_state: State = (seed_caller, RequiredPred::Any);
    queue.push_back(seed_state);
    visited.insert(seed_state);

    while let Some((node, required)) = queue.pop_front() {
        let Some(preds) = reverse.get(node.index()) else {
            continue;
        };
        for &edge_idx in preds {
            let edge = &facts.edges[edge_idx];
            match required {
                RequiredPred::Any => {}
                RequiredPred::Edge(req_ei) => {
                    if edge_idx != req_ei {
                        continue;
                    }
                }
                RequiredPred::Caller(req_caller) => {
                    if edge.caller != req_caller {
                        continue;
                    }
                }
            }
            let pred = edge.caller;
            let next_req = edge
                .context_from_edge
                .map(RequiredPred::Edge)
                .or_else(|| edge.context_from_caller.map(RequiredPred::Caller))
                .unwrap_or(RequiredPred::Any);
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
                if is_non_production_entry_path(&pred_fn.file) {
                    queue.push_back(pred_state);
                }
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

/// Cheap, idempotent path ops (`mkdir(exist_ok=True)`, `makedirs(exist_ok=True)`,
/// `unlink(missing_ok=True)`) grade as candidates; the flag must be a literal
/// `True` (keyword, or its stdlib positional slot). `Path.touch` is a candidate
/// in the sink table itself.
pub(super) fn is_cheap_path_op(qualname: &str, edge: &CallEdge) -> bool {
    let (flag, positional) = match qualname {
        "pathlib.Path.mkdir" | "os.makedirs" => ("exist_ok", 2),
        "pathlib.Path.unlink" => ("missing_ok", 0),
        _ => return false,
    };
    edge.keyword_literals
        .iter()
        .any(|(k, v)| k == flag && *v == Some(LiteralArg::True))
        || edge.positional_literals.get(positional).copied().flatten() == Some(LiteralArg::True)
}

/// `functools.cache` / `functools.lru_cache` decorated function: its body only
/// runs on a cache miss.
pub(super) fn is_memoized(facts: &ProjectFacts, fid: FunctionId) -> bool {
    facts.decorator_refs.get(&fid).is_some_and(|refs| {
        refs.iter().any(|r| {
            matches!(r, DecoratorRef::External(q)
                if matches!(q.as_str(), "functools.cache" | "functools.lru_cache"))
        })
    })
}

pub(super) fn build_class_inits(facts: &ProjectFacts) -> ClassInitIndex {
    let mut out: ClassInitIndex = HashMap::new();
    for f in &facts.functions {
        if f.is_third_party || f.file.is_empty() {
            continue;
        }
        if let Some(class_qual) = f.qualname.strip_suffix(".__init__") {
            out.entry((f.file.clone(), class_qual.to_string()))
                .or_default()
                .push((f.file.clone(), f.line, f.end_line.max(f.line)));
        }
    }
    out
}

fn push_function_spans(
    facts: &ProjectFacts,
    class_inits: &ClassInitIndex,
    fid: FunctionId,
    out: &mut Vec<(String, u32, u32)>,
) {
    let f = facts.function(fid);
    if f.is_third_party || f.file.is_empty() {
        return;
    }
    out.push((f.file.clone(), f.line, f.end_line.max(f.line)));
    if let Some((class_qual, _method)) = f.qualname.rsplit_once('.')
        && let Some(inits) = class_inits.get(&(f.file.clone(), class_qual.to_string()))
    {
        out.extend(inits.iter().cloned());
    }
}

pub(super) fn collect_item_spans(
    facts: &ProjectFacts,
    class_inits: &ClassInitIndex,
    path: &PathToEntry,
    terminal_edge: &CallEdge,
) -> Vec<(String, u32, u32)> {
    let mut spans = Vec::new();
    push_function_spans(facts, class_inits, path.entry, &mut spans);
    for &eidx in &path.edges {
        let e = &facts.edges[eidx];
        push_function_spans(facts, class_inits, e.caller, &mut spans);
        if let CalleeRef::Project(fid) = e.callee {
            push_function_spans(facts, class_inits, fid, &mut spans);
        }
        if let Some(bi) = e.context_from_edge {
            if let Some(be) = facts.edges.get(bi) {
                push_function_spans(facts, class_inits, be.caller, &mut spans);
            }
        } else if let Some(binder) = e.context_from_caller {
            push_function_spans(facts, class_inits, binder, &mut spans);
        }
    }
    push_function_spans(facts, class_inits, terminal_edge.caller, &mut spans);
    if let CalleeRef::Project(fid) = terminal_edge.callee {
        push_function_spans(facts, class_inits, fid, &mut spans);
    }
    for &fid in terminal_edge.positional_callables.iter().flatten() {
        push_function_spans(facts, class_inits, fid, &mut spans);
    }
    for (_, fid) in &terminal_edge.keyword_callables {
        push_function_spans(facts, class_inits, *fid, &mut spans);
    }
    spans.sort();
    spans.dedup();
    spans
}
