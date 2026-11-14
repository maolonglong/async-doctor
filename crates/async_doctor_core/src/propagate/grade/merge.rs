//! Post-propagation finding mergers (`open` + `read`/`write`, same-line ctor + method).

use std::collections::{HashMap, HashSet};

use super::ItemSpanMap;
use crate::report::{ChainStep, Finding};

fn absorb_spans(item_spans: &mut ItemSpanMap, dst_id: &str, src_id: &str) {
    if let Some(src_spans) = item_spans.remove(src_id) {
        let dst = item_spans.entry(dst_id.to_string()).or_default();
        dst.extend(src_spans);
        dst.sort();
        dst.dedup();
    }
}

/// Merge `open(...)` + subsequent `f.read()`/`f.write()` findings for the same entry.
pub(in crate::propagate) fn merge_file_io_findings(
    findings: Vec<Finding>,
    item_spans: &mut ItemSpanMap,
) -> Vec<Finding> {
    let mut opens: Vec<Finding> = Vec::new();
    let mut others: Vec<Finding> = Vec::new();
    for f in findings {
        let sink_text = f.chain.last().map(|c| c.text.as_str()).unwrap_or("");
        // `open(p).read()` is the rw hop of the inner `open(p)` call on the
        // same line, not a second open.
        let chained_open_rw =
            sink_text.trim_start().starts_with("open(") && is_file_rw_text(sink_text);
        if is_open_call_text(sink_text) && !chained_open_rw
            || f.fix.fix_group_id.contains("open") && f.rule == "blocking-file"
        {
            // Prefer matching by last chain hop looking like open(
            if is_open_call_text(sink_text) && !chained_open_rw {
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

    // Absorb rw only when the call-path prefix matches (same hops before the
    // sink). Prevents mashing `_check` and `_patch` open sites into one chain
    // while still collapsing `open`+`read`/`write` for a single with-block.
    // When several opens share a prefix, bind each rw to the nearest preceding
    // open by sink line (so write after a second open is not glued to the first).
    // Snapshot prefix/line *before* absorption — pushing rw hops onto `open.chain`
    // would otherwise change the terminal hop and block later reads/writes.
    struct OpenMeta {
        prefix: Vec<(String, u32, String)>,
        line: u32,
        file: String,
        name: String,
    }
    let open_meta: Vec<OpenMeta> = opens
        .iter()
        .map(|o| OpenMeta {
            prefix: chain_prefix_before_sink(&o.chain),
            line: o.chain.last().map(|c| c.line).unwrap_or(0),
            file: o.entry.file.clone(),
            name: o.entry.name.clone(),
        })
        .collect();
    let mut opens_by_entry: HashMap<(&str, &str), Vec<usize>> = HashMap::new();
    for (oi, meta) in open_meta.iter().enumerate() {
        opens_by_entry
            .entry((meta.file.as_str(), meta.name.as_str()))
            .or_default()
            .push(oi);
    }
    let mut absorbed: HashSet<usize> = HashSet::new();
    for (i, rw) in others.iter().enumerate() {
        if rw.rule != "blocking-file"
            || !is_file_rw_text(rw.chain.last().map(|c| c.text.as_str()).unwrap_or(""))
        {
            continue;
        }
        let Some(candidate_opens) =
            opens_by_entry.get(&(rw.entry.file.as_str(), rw.entry.name.as_str()))
        else {
            continue;
        };
        let rw_line = rw.chain.last().map(|c| c.line).unwrap_or(0);
        let rw_prefix = chain_prefix_before_sink(&rw.chain);
        let mut best: Option<usize> = None;
        for &oi in candidate_opens {
            let meta = &open_meta[oi];
            if meta.prefix != rw_prefix {
                continue;
            }
            if meta.line > rw_line {
                continue;
            }
            let better = match best {
                None => true,
                Some(bi) => meta.line >= open_meta[bi].line,
            };
            if better {
                best = Some(oi);
            }
        }
        if let Some(oi) = best {
            for step in &rw.chain {
                if !opens[oi]
                    .chain
                    .iter()
                    .any(|s| s.file == step.file && s.line == step.line && s.text == step.text)
                {
                    opens[oi].chain.push(step.clone());
                }
            }
            absorb_spans(item_spans, &opens[oi].id, &rw.id);
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

fn is_open_call_text(text: &str) -> bool {
    let t = text.trim();
    t.starts_with("open(")
        || t.contains(".open(")
        || t.contains("ZipFile(")
        || t.contains("TarFile(")
}

/// Chain hops before the terminal sink (empty when the sink is inside the entry).
fn chain_prefix_before_sink(chain: &[ChainStep]) -> Vec<(String, u32, String)> {
    if chain.len() <= 1 {
        return Vec::new();
    }
    chain[..chain.len() - 1]
        .iter()
        .map(|s| (s.file.clone(), s.line, s.text.clone()))
        .collect()
}

fn is_file_rw_text(text: &str) -> bool {
    let t = text.trim();
    t.contains(".read(")
        || t.contains(".readline(")
        || t.contains(".readlines(")
        || t.contains(".write(")
        || t.contains(".writelines(")
        || t.contains(".writestr(")
        || t.contains(".extract(")
        || t.contains(".extractall(")
        || t.contains(".add(")
        || t.contains(".addfile(")
}

/// Collapse `SMTP(...).sendmail(...)` / `ZipFile(...).extractall()` into one
/// finding: keep the method sink, absorb the same-line constructor hop.
pub(in crate::propagate) fn merge_same_line_ctor_call_findings(
    findings: Vec<Finding>,
    item_spans: &mut ItemSpanMap,
) -> Vec<Finding> {
    if findings.len() < 2 {
        return findings;
    }
    let n = findings.len();
    let mut buckets: HashMap<(&str, &str, &str, u32), Vec<usize>> = HashMap::new();
    for (i, f) in findings.iter().enumerate() {
        if let Some(sink) = f.chain.last() {
            buckets
                .entry((
                    f.entry.file.as_str(),
                    f.entry.name.as_str(),
                    sink.file.as_str(),
                    sink.line,
                ))
                .or_default()
                .push(i);
        }
    }
    let mut drop_ctor: HashSet<usize> = HashSet::new();
    let mut absorb_into: HashMap<usize, Vec<usize>> = HashMap::new();
    for group in buckets.values() {
        if group.len() < 2 {
            continue;
        }
        for (pos_a, &i) in group.iter().enumerate() {
            if drop_ctor.contains(&i) {
                continue;
            }
            let Some(i_sink) = findings[i].chain.last() else {
                continue;
            };
            for &j in &group[(pos_a + 1)..] {
                if drop_ctor.contains(&j) {
                    continue;
                }
                let Some(j_sink) = findings[j].chain.last() else {
                    continue;
                };
                let (method_i, ctor_i) = if i_sink.text.contains(j_sink.text.as_str())
                    && i_sink.text.len() > j_sink.text.len()
                    && looks_like_ctor_call_text(&j_sink.text)
                {
                    (i, j)
                } else if j_sink.text.contains(i_sink.text.as_str())
                    && j_sink.text.len() > i_sink.text.len()
                    && looks_like_ctor_call_text(&i_sink.text)
                {
                    (j, i)
                } else {
                    continue;
                };
                drop_ctor.insert(ctor_i);
                absorb_into.entry(method_i).or_default().push(ctor_i);
            }
        }
    }
    if drop_ctor.is_empty() {
        return findings;
    }
    let ctor_chains: HashMap<usize, (String, Vec<ChainStep>)> = drop_ctor
        .iter()
        .map(|&i| (i, (findings[i].id.clone(), findings[i].chain.clone())))
        .collect();
    let mut out = Vec::with_capacity(n - drop_ctor.len());
    for (i, mut f) in findings.into_iter().enumerate() {
        if drop_ctor.contains(&i) {
            continue;
        }
        if let Some(ctors) = absorb_into.get(&i) {
            // Keep the method sink terminal (comment contract / actionable site).
            let method_sink = f.chain.pop();
            for &ci in ctors {
                let (ctor_id, ctor_steps) = &ctor_chains[&ci];
                absorb_spans(item_spans, &f.id, ctor_id);
                for step in ctor_steps {
                    if !f
                        .chain
                        .iter()
                        .any(|s| s.file == step.file && s.line == step.line && s.text == step.text)
                        && method_sink.as_ref().is_none_or(|m| {
                            !(m.file == step.file && m.line == step.line && m.text == step.text)
                        })
                    {
                        f.chain.push(step.clone());
                    }
                }
            }
            if let Some(sink) = method_sink {
                f.why = format!("async-doctor why {}:{}", sink.file, sink.line);
                f.chain.push(sink);
            }
        }
        out.push(f);
    }
    out
}

fn looks_like_ctor_call_text(text: &str) -> bool {
    let t = text.trim();
    // `smtplib.SMTP(...)` / `zipfile.ZipFile(...)` — capitalized type callee.
    let Some(open) = t.find('(') else {
        return false;
    };
    let head = &t[..open];
    let method = head.rsplit('.').next().unwrap_or(head);
    method.chars().next().is_some_and(|c| c.is_uppercase())
}
