//! Optional pre-follow fact dumps for cache-identity debugging.
//!
//! Enable with `ASYNC_DOCTOR_DUMP_FACTS=1`.

use std::collections::HashMap;

use async_doctor_core::{CalleeRef, ProjectFacts};

use crate::sitepkgs::FollowTarget;

pub(crate) fn dump_facts_pre_follow(
    label: &str,
    facts: &ProjectFacts,
    follow_targets: &HashMap<String, FollowTarget>,
) {
    if std::env::var_os("ASYNC_DOCTOR_DUMP_FACTS").is_none() {
        return;
    }
    let project = facts.functions.iter().filter(|f| !f.is_third_party).count();
    let mut fn_parts: Vec<String> = facts
        .functions
        .iter()
        .filter(|f| !f.is_third_party)
        .map(|f| {
            format!(
                "{}:{}:{}:a{}:g{}:sr{}:l{:?}:ow{}:params={:?}:inv={:?}:twins={:?}:file={}",
                f.module,
                f.qualname,
                f.line,
                f.is_async as u8,
                f.is_generator as u8,
                f.is_sync_route as u8,
                f.label,
                f.is_offload_wrapper as u8,
                f.param_names,
                f.invokes_params,
                f.async_twins,
                f.file,
            )
        })
        .collect();
    fn_parts.sort();
    let mut edge_parts: Vec<String> = facts
        .edges
        .iter()
        .map(|e| {
            let callee = match &e.callee {
                CalleeRef::Project(id) => {
                    let f = &facts.functions[id.index()];
                    format!("P:{}:{}", f.module, f.qualname)
                }
                CalleeRef::External { qualname } => format!("E:{qualname}"),
                CalleeRef::Unresolved { reason } => format!("U:{reason}"),
                CalleeRef::Offload(k) => format!("O:{}", k.as_str()),
                CalleeRef::ParamInvoke { index } => format!("I:{index}"),
            };
            let caller = &facts.functions[e.caller.index()];
            let conds: Vec<_> = e.conditions.iter().map(|c| c.as_str()).collect();
            let pos_c: Vec<String> = e
                .positional_callables
                .iter()
                .map(|o| match o {
                    Some(id) => {
                        let f = &facts.functions[id.index()];
                        format!("{}:{}", f.module, f.qualname)
                    }
                    None => "-".into(),
                })
                .collect();
            let kw_c: Vec<String> = e
                .keyword_callables
                .iter()
                .map(|(k, id)| {
                    let f = &facts.functions[id.index()];
                    format!("{}={}:{}", k, f.module, f.qualname)
                })
                .collect();
            format!(
                "{}:{}:{}:{}:{}:{:?}:aw{}:conds={:?}:lits={:?}:eager{}:ow{}:coro{:?}:pg{:?}:posc={pos_c:?}:kwc={kw_c:?}:ctx{:?}",
                caller.module,
                caller.qualname,
                callee,
                e.line,
                e.occurrence,
                e.kind,
                e.is_awaited as u8,
                conds,
                e.positional_literals,
                e.is_eager_offload_arg as u8,
                e.is_offload_wrapper as u8,
                e.callee_is_coroutine,
                e.param_guard,
                e.context_from_caller.map(|id| {
                    let f = &facts.functions[id.index()];
                    format!("{}:{}", f.module, f.qualname)
                }),
            )
        })
        .collect();
    edge_parts.sort();
    let hash_parts = |parts: &[String]| -> u64 {
        let mut hash: u64 = 0xcbf29ce484222325;
        for p in parts {
            for b in p.as_bytes() {
                hash ^= u64::from(*b);
                hash = hash.wrapping_mul(0x100000001b3);
            }
        }
        hash
    };
    let mut ft_parts: Vec<String> = follow_targets
        .iter()
        .map(|(k, t)| format!("{}:{}:{}:{}", k, t.qualname, t.path.as_str(), t.name_start))
        .collect();
    ft_parts.sort();
    let mut only_twins: Vec<String> = facts
        .functions
        .iter()
        .filter(|f| !f.is_third_party && !f.async_twins.is_empty())
        .map(|f| format!("{}:{}:{:?}", f.module, f.qualname, f.async_twins))
        .collect();
    only_twins.sort();
    let mut only_inv: Vec<String> = facts
        .functions
        .iter()
        .filter(|f| !f.is_third_party && !f.invokes_params.is_empty())
        .map(|f| format!("{}:{}:{:?}", f.module, f.qualname, f.invokes_params))
        .collect();
    only_inv.sort();
    let fn_hash = hash_parts(&fn_parts);
    let edge_hash = hash_parts(&edge_parts);
    let ft_hash = hash_parts(&ft_parts);
    let twin_hash = hash_parts(&only_twins);
    let inv_hash = hash_parts(&only_inv);
    let async_entries = facts
        .functions
        .iter()
        .filter(|f| !f.is_third_party && f.is_async && !f.is_sync_route)
        .count();
    let mut tp_parts: Vec<String> = facts
        .functions
        .iter()
        .filter(|f| f.is_third_party)
        .map(|f| {
            format!(
                "{}:{}:a{}:g{}:file={}:pkg={:?}:ver={:?}",
                f.module,
                f.qualname,
                f.is_async as u8,
                f.is_generator as u8,
                f.file,
                f.package_name,
                f.package_version,
            )
        })
        .collect();
    tp_parts.sort();
    let tp_hash = hash_parts(&tp_parts);
    let tp_async = facts
        .functions
        .iter()
        .filter(|f| f.is_third_party && f.is_async)
        .count();
    eprintln!(
        "async-doctor facts-pre-follow ({label}): fns={} (first_party={project}) async_entries={async_entries} edges={} follow_targets={} unresolved_tp={} unresolved_imports={} fn_hash={fn_hash:016x} edge_hash={edge_hash:016x} ft_hash={ft_hash:016x} twin_hash={twin_hash:016x} inv_hash={inv_hash:016x} tp_hash={tp_hash:016x} tp_async={tp_async} twins={} invs={}",
        facts.functions.len(),
        facts.edges.len(),
        follow_targets.len(),
        facts.unresolved_third_party_modules.len(),
        facts.unresolved_imports.len(),
        only_twins.len(),
        only_inv.len(),
    );

    // Optional full edge/function dump for identity diffs (large).
    if let Ok(path) = std::env::var("ASYNC_DOCTOR_DUMP_FACTS_FILE") {
        let out = format!("{path}.{label}.txt");
        let mut body = String::new();
        body.push_str("---functions---\n");
        for p in &fn_parts {
            body.push_str(p);
            body.push('\n');
        }
        for p in &tp_parts {
            body.push_str("TP:");
            body.push_str(p);
            body.push('\n');
        }
        body.push_str("---edges---\n");
        for p in &edge_parts {
            body.push_str(p);
            body.push('\n');
        }
        let _ = std::fs::write(&out, body);
        eprintln!("async-doctor wrote facts dump {out}");
    }
}
