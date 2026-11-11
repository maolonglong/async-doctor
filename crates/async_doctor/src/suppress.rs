//! Inline `# async-doctor: ignore[rule]: reason` suppressions.
//!
//! Zero-config: comments only, no config file. A reason is required. Unused
//! suppressions are reported so agents do not leave dead ignores behind.
//! Comment detection uses a whole-file Python lexical scanner so `#` inside
//! single- or multi-line string literals is ignored, trailing ignores on the
//! closing line of multi-line calls match the call start line, and standalone
//! ignores above `@decorator` stacks match the decorated `async def`.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use async_doctor_core::{Report, UnusedSuppression};

use crate::paths::{normalize_path, paths_equal};

/// One suppression comment in source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Suppression {
    pub file: String,
    pub line: u32,
    pub rule: String,
    pub reason: String,
    /// Additional target line after skipping `@decorator` lines below a
    /// standalone ignore comment.
    pub decorated_def_line: Option<u32>,
    /// Opening lines of bracketed expressions that closed on `line`, plus
    /// `(stmt_start..=line)` when a multi-line statement finishes at depth 0.
    pub covered_lines: Vec<u32>,
}

/// An `ignore[rule]` comment that omitted the required reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MissingReasonIgnore {
    pub file: String,
    pub line: u32,
    pub rule: String,
}

/// Result of applying suppressions to a report.
#[derive(Debug)]
pub struct SuppressOutcome {
    pub report: Report,
    /// Findings removed by a matching ignore comment.
    pub suppressed_findings: u32,
    pub missing_reason: Vec<MissingReasonIgnore>,
}

/// Parse suppressions under `root`, apply them to `report`, and collect unused.
#[allow(dead_code)]
pub fn apply(report: Report, root: &Path) -> anyhow::Result<SuppressOutcome> {
    apply_scoped(report, root, None)
}

/// Parse suppressions under `root`, apply them to `report`, and collect unused.
///
/// When `single_file` is `Some(target)`, discovery does NOT walk sibling `.py`
/// files in `root`: only `target` and chain-hop files referenced by `report`
/// are read, and `unused_suppressions` / `missing_reason` are reported strictly
/// for `target`.
pub fn apply_scoped(
    mut report: Report,
    root: &Path,
    single_file: Option<&Path>,
) -> anyhow::Result<SuppressOutcome> {
    let single_target = single_file.map(|f| {
        let canon = dunce::canonicalize(f).unwrap_or_else(|_| f.to_path_buf());
        let rel = relativize(root, &canon);
        (canon, rel)
    });
    let files = files_to_scan(
        &report,
        root,
        single_target.as_ref().map(|(p, _)| p.as_path()),
    )?;
    let mut by_file: HashMap<String, Vec<Suppression>> = HashMap::new();
    let mut missing_reason = Vec::new();
    let scan_one = |file: &PathBuf| -> Option<(String, ParseResult)> {
        let text = fs::read_to_string(file).ok()?;
        if !text.contains("async-doctor") {
            return None;
        }
        let rel = relativize(root, file);
        let parsed = parse_file(&rel, &text);
        if parsed.suppressions.is_empty() && parsed.missing_reason.is_empty() {
            return None;
        }
        Some((rel, parsed))
    };
    let hits: Vec<(String, ParseResult)> = if files.len() >= 64 {
        let nthreads = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1)
            .min(8);
        let chunk_size = files.len().div_ceil(nthreads).max(1);
        std::thread::scope(|s| {
            let handles: Vec<_> = files
                .chunks(chunk_size)
                .map(|chunk| s.spawn(move || chunk.iter().filter_map(scan_one).collect::<Vec<_>>()))
                .collect();
            let mut out = Vec::new();
            for h in handles {
                out.extend(h.join().expect("suppress worker"));
            }
            out
        })
    } else {
        files.iter().filter_map(scan_one).collect()
    };
    for (rel, parsed) in hits {
        if single_target
            .as_ref()
            .is_none_or(|(_, target_rel)| paths_equal(&rel, target_rel))
        {
            missing_reason.extend(parsed.missing_reason);
        }
        if !parsed.suppressions.is_empty() {
            by_file.insert(rel, parsed.suppressions);
        }
    }

    if by_file.is_empty() {
        missing_reason.sort_by(|a, b| (&a.file, a.line, &a.rule).cmp(&(&b.file, b.line, &b.rule)));
        report.unused_suppressions = Vec::new();
        return Ok(SuppressOutcome {
            report,
            suppressed_findings: 0,
            missing_reason,
        });
    }

    let mut used: HashSet<(String, u32, String)> = HashSet::new();
    let mut findings = Vec::new();
    let mut suppressed_findings = 0u32;
    for f in std::mem::take(&mut report.findings) {
        if let Some(sup) =
            matching_suppression(&f.rule, &f.entry.file, f.entry.line, &f.chain, &by_file)
        {
            used.insert((sup.file.clone(), sup.line, sup.rule.clone()));
            suppressed_findings += 1;
            continue;
        }
        findings.push(f);
    }
    let mut candidates = Vec::new();
    for c in std::mem::take(&mut report.candidates) {
        if let Some(sup) =
            matching_suppression(&c.rule, &c.entry.file, c.entry.line, &c.chain, &by_file)
        {
            used.insert((sup.file.clone(), sup.line, sup.rule.clone()));
            continue;
        }
        candidates.push(c);
    }
    let mut advice = Vec::new();
    for a in std::mem::take(&mut report.advice) {
        if let Some(sup) =
            matching_suppression(&a.rule, &a.entry.file, a.entry.line, &a.chain, &by_file)
        {
            used.insert((sup.file.clone(), sup.line, sup.rule.clone()));
            continue;
        }
        advice.push(a);
    }

    let mut unused = Vec::new();
    for sups in by_file.values() {
        for s in sups {
            if let Some((_, ref target_rel)) = single_target
                && !paths_equal(&s.file, target_rel)
            {
                continue;
            }
            if !used.contains(&(s.file.clone(), s.line, s.rule.clone())) {
                unused.push(UnusedSuppression {
                    file: s.file.clone(),
                    line: s.line,
                    rule: s.rule.clone(),
                    reason: s.reason.clone(),
                });
            }
        }
    }
    unused.sort_by(|a, b| (&a.file, a.line, &a.rule).cmp(&(&b.file, b.line, &b.rule)));
    missing_reason.sort_by(|a, b| (&a.file, a.line, &a.rule).cmp(&(&b.file, b.line, &b.rule)));

    report.findings = findings;
    report.candidates = candidates;
    report.advice = advice;
    report.unused_suppressions = unused;
    Ok(SuppressOutcome {
        report,
        suppressed_findings,
        missing_reason,
    })
}

fn matching_suppression<'a>(
    rule: &str,
    entry_file: &str,
    entry_line: u32,
    chain: &[async_doctor_core::ChainStep],
    by_file: &'a HashMap<String, Vec<Suppression>>,
) -> Option<&'a Suppression> {
    let mut locs: Vec<(&str, u32)> = vec![(entry_file, entry_line)];
    for step in chain {
        locs.push((&step.file, step.line));
    }
    for (file, line) in locs {
        let key = normalize_path(file);
        let Some(sups) = by_file.get(&key).or_else(|| {
            by_file
                .iter()
                .find(|(k, _)| paths_equal(k, &key))
                .map(|(_, v)| v)
        }) else {
            continue;
        };
        for s in sups {
            if s.rule != rule {
                continue;
            }
            if s.matches_line(line) {
                return Some(s);
            }
        }
    }
    None
}

impl Suppression {
    fn matches_line(&self, line: u32) -> bool {
        self.line == line
            || self.line + 1 == line
            || self.decorated_def_line == Some(line)
            || self.covered_lines.contains(&line)
    }
}

#[derive(Debug, Default)]
struct ParseResult {
    suppressions: Vec<Suppression>,
    missing_reason: Vec<MissingReasonIgnore>,
}

#[derive(Debug, Default, Clone)]
struct LineScanInfo {
    has_code_before_comment: bool,
    is_decorator_line: bool,
    comment_range: Option<(usize, usize)>,
    depth_at_comment: usize,
    stmt_start_at_comment: Option<u32>,
    closed_open_lines: Vec<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LexState {
    Code,
    SingleString { quote: u8 },
    TripleString { quote: u8 },
}

fn parse_file(file: &str, source: &str) -> ParseResult {
    let lines = scan_python_lines(source);
    let norm_file = normalize_path(file);
    let mut out = ParseResult::default();

    for idx in 1..lines.len() {
        let line_no = idx as u32;
        let info = &lines[idx];
        let Some((start, end)) = info.comment_range else {
            continue;
        };
        let comment = source[start..end].trim();
        match parse_ignore_comment(comment) {
            IgnoreParse::Suppression { rule, reason } => {
                let decorated_def_line = if !info.has_code_before_comment {
                    find_decorated_target_line(&lines, idx)
                } else {
                    None
                };
                let mut covered_lines = Vec::new();
                if info.has_code_before_comment {
                    for &open_line in &info.closed_open_lines {
                        if open_line < line_no && !covered_lines.contains(&open_line) {
                            covered_lines.push(open_line);
                        }
                    }
                    // Multi-line statement finishing at bracket depth 0 (e.g.
                    // `client.users(\n...\n).fetch(\n...\n)  # ignore`): cover
                    // the statement's starting line as well.
                    if info.depth_at_comment == 0
                        && info.closed_open_lines.iter().any(|&l| l < line_no)
                        && let Some(stmt_start) = info.stmt_start_at_comment
                    {
                        for l in stmt_start..line_no {
                            if !covered_lines.contains(&l) {
                                covered_lines.push(l);
                            }
                        }
                    }
                }
                out.suppressions.push(Suppression {
                    file: norm_file.clone(),
                    line: line_no,
                    rule,
                    reason,
                    decorated_def_line,
                    covered_lines,
                });
            }
            IgnoreParse::MissingReason { rule } => {
                out.missing_reason.push(MissingReasonIgnore {
                    file: norm_file.clone(),
                    line: line_no,
                    rule,
                });
            }
            IgnoreParse::None => {}
        }
    }
    out
}

/// If standalone comment at `comment_line` is immediately followed by one or
/// more `@decorator` lines (including multi-line `@dec(...)` spans), return the
/// line number of the `def` / `async def` immediately after the decorator block.
fn find_decorated_target_line(lines: &[LineScanInfo], comment_line: usize) -> Option<u32> {
    let mut cursor = comment_line + 1;
    let mut saw_decorator = false;
    while cursor < lines.len() {
        let info = &lines[cursor];
        if info.is_decorator_line {
            saw_decorator = true;
            cursor += 1;
            continue;
        }
        // Skip comment-only lines nestled between decorators.
        if !info.has_code_before_comment && info.comment_range.is_some() && saw_decorator {
            cursor += 1;
            continue;
        }
        break;
    }
    if saw_decorator && cursor < lines.len() {
        Some(cursor as u32)
    } else {
        None
    }
}

/// Single-pass whole-file Python lexer tracking string literals across lines,
/// bracket nesting, logical statement starts, and `@decorator` spans.
///
/// Returned Vec is 1-indexed (`lines[1]` is line 1).
fn scan_python_lines(source: &str) -> Vec<LineScanInfo> {
    let bytes = source.as_bytes();
    let total_lines = bytes.iter().filter(|&&b| b == b'\n').count() + 2;
    let mut lines = vec![LineScanInfo::default(); total_lines];

    let mut state = LexState::Code;
    let mut line_no: usize = 1;
    let mut first_token_on_line = true;
    let mut bracket_stack: Vec<(u8, u32)> = Vec::new();
    let mut current_stmt_start: Option<u32> = None;
    let mut in_decorator_stmt = false;
    let mut line_continuation = false;

    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        match state {
            LexState::SingleString { quote } => {
                if b == b'\\' {
                    if i + 1 < bytes.len() {
                        if bytes[i + 1] == b'\n' {
                            line_no += 1;
                            first_token_on_line = false;
                        }
                        i += 2;
                    } else {
                        i += 1;
                    }
                    continue;
                }
                if b == quote {
                    state = LexState::Code;
                    i += 1;
                    continue;
                }
                if b == b'\n' {
                    // Unterminated single-line string: recover at newline.
                    state = LexState::Code;
                    line_no += 1;
                    first_token_on_line = true;
                    if bracket_stack.is_empty() {
                        current_stmt_start = None;
                        in_decorator_stmt = false;
                    }
                    i += 1;
                    continue;
                }
                i += 1;
            }
            LexState::TripleString { quote } => {
                if b == b'\\' {
                    if i + 1 < bytes.len() {
                        if bytes[i + 1] == b'\n' {
                            line_no += 1;
                            first_token_on_line = false;
                        }
                        i += 2;
                    } else {
                        i += 1;
                    }
                    continue;
                }
                if matches_triple(bytes, i, quote) {
                    state = LexState::Code;
                    i += 3;
                    continue;
                }
                if b == b'\n' {
                    line_no += 1;
                    first_token_on_line = false;
                    i += 1;
                    continue;
                }
                i += 1;
            }
            LexState::Code => {
                if b == b'\n' {
                    if !line_continuation && bracket_stack.is_empty() {
                        current_stmt_start = None;
                        in_decorator_stmt = false;
                    }
                    line_continuation = false;
                    line_no += 1;
                    first_token_on_line = true;
                    if in_decorator_stmt {
                        lines[line_no].is_decorator_line = true;
                    }
                    i += 1;
                    continue;
                }
                if b == b' ' || b == b'\t' || b == b'\r' || b == 0x0c {
                    i += 1;
                    continue;
                }
                if b == b'\\' {
                    line_continuation = true;
                    i += 1;
                    continue;
                }
                if b == b'#' {
                    let start = i + 1;
                    let mut end = start;
                    while end < bytes.len() && bytes[end] != b'\n' {
                        end += 1;
                    }
                    let trimmed_end = if end > start && bytes[end - 1] == b'\r' {
                        end - 1
                    } else {
                        end
                    };
                    let info = &mut lines[line_no];
                    info.comment_range = Some((start, trimmed_end));
                    info.depth_at_comment = bracket_stack.len();
                    info.stmt_start_at_comment = current_stmt_start;
                    i = end;
                    continue;
                }

                line_continuation = false;
                lines[line_no].has_code_before_comment = true;
                if current_stmt_start.is_none() {
                    current_stmt_start = Some(line_no as u32);
                }
                if first_token_on_line {
                    first_token_on_line = false;
                    if b == b'@' && bracket_stack.is_empty() {
                        in_decorator_stmt = true;
                        lines[line_no].is_decorator_line = true;
                    } else if in_decorator_stmt {
                        lines[line_no].is_decorator_line = true;
                    }
                }

                if let Some((quote_pos, quote, is_triple)) = string_start(bytes, i) {
                    state = if is_triple {
                        i = quote_pos + 3;
                        LexState::TripleString { quote }
                    } else {
                        i = quote_pos + 1;
                        LexState::SingleString { quote }
                    };
                    continue;
                }

                match b {
                    b'(' | b'[' | b'{' => {
                        bracket_stack.push((b, line_no as u32));
                    }
                    b')' | b']' | b'}' => {
                        let expected = match b {
                            b')' => b'(',
                            b']' => b'[',
                            _ => b'{',
                        };
                        if let Some(pos) = bracket_stack
                            .iter()
                            .rposition(|(open, _)| *open == expected)
                        {
                            let (_, open_line) = bracket_stack[pos];
                            bracket_stack.truncate(pos);
                            lines[line_no].closed_open_lines.push(open_line);
                        }
                    }
                    b';' if bracket_stack.is_empty() => {
                        current_stmt_start = None;
                        in_decorator_stmt = false;
                    }
                    _ => {}
                }
                i += 1;
            }
        }
    }

    lines
}

fn matches_triple(bytes: &[u8], i: usize, quote: u8) -> bool {
    i + 3 <= bytes.len() && bytes[i] == quote && bytes[i + 1] == quote && bytes[i + 2] == quote
}

fn string_start(bytes: &[u8], i: usize) -> Option<(usize, u8, bool)> {
    let mut j = i;
    while j < bytes.len()
        && matches!(
            bytes[j],
            b'r' | b'R' | b'f' | b'F' | b'b' | b'B' | b'u' | b'U'
        )
    {
        j += 1;
    }
    if j < bytes.len() && (bytes[j] == b'\'' || bytes[j] == b'"') {
        if j > i && i > 0 && (bytes[i - 1].is_ascii_alphanumeric() || bytes[i - 1] == b'_') {
            return None;
        }
        let q = bytes[j];
        let triple = matches_triple(bytes, j, q);
        return Some((j, q, triple));
    }
    None
}

enum IgnoreParse {
    Suppression { rule: String, reason: String },
    MissingReason { rule: String },
    None,
}

fn parse_ignore_comment(comment: &str) -> IgnoreParse {
    let rest = match comment
        .strip_prefix("async-doctor:")
        .or_else(|| comment.strip_prefix("async-doctor :"))
    {
        Some(r) => r.trim_start(),
        None => return IgnoreParse::None,
    };
    let rest = match rest.strip_prefix("ignore") {
        Some(r) => r.trim_start(),
        None => return IgnoreParse::None,
    };
    if !rest.starts_with('[') {
        return IgnoreParse::None;
    }
    let Some(end) = rest.find(']') else {
        return IgnoreParse::None;
    };
    let rule = rest[1..end].trim();
    if rule.is_empty() {
        return IgnoreParse::None;
    }
    let after = rest[end + 1..].trim_start();
    match after.strip_prefix(':') {
        Some(reason) => {
            let reason = reason.split(" # ").next().unwrap_or(reason).trim();
            if reason.is_empty() {
                IgnoreParse::MissingReason {
                    rule: rule.to_string(),
                }
            } else {
                IgnoreParse::Suppression {
                    rule: rule.to_string(),
                    reason: reason.to_string(),
                }
            }
        }
        None => IgnoreParse::MissingReason {
            rule: rule.to_string(),
        },
    }
}

fn files_to_scan<'a>(
    report: &'a Report,
    root: &Path,
    single_file: Option<&Path>,
) -> anyhow::Result<Vec<PathBuf>> {
    let mut paths: HashSet<PathBuf> = if let Some(target) = single_file {
        let mut set = HashSet::new();
        if target.is_file() {
            set.insert(target.to_path_buf());
        }
        set
    } else if let Some(candidates) = async_doctor_ty::take_ignore_candidate_files(root) {
        candidates.into_iter().collect()
    } else {
        async_doctor_ty::discover_python_files(root)?
            .into_iter()
            .collect()
    };

    let mut rel_seen: HashSet<&'a str> = HashSet::new();
    let mut add = |paths: &mut HashSet<PathBuf>, rel: &'a str| {
        if !rel.is_empty() && rel_seen.insert(rel) {
            let p = root.join(rel);
            if !paths.contains(&p) && p.is_file() {
                paths.insert(p);
            }
        }
    };
    for f in &report.findings {
        add(&mut paths, &f.entry.file);
        for s in &f.chain {
            add(&mut paths, &s.file);
        }
    }
    for c in &report.candidates {
        add(&mut paths, &c.entry.file);
        for s in &c.chain {
            add(&mut paths, &s.file);
        }
    }
    for a in &report.advice {
        add(&mut paths, &a.entry.file);
        for s in &a.chain {
            add(&mut paths, &s.file);
        }
    }
    Ok(paths.into_iter().collect())
}

fn relativize(root: &Path, file: &Path) -> String {
    file.strip_prefix(root)
        .map(|p| normalize_path(&p.to_string_lossy()))
        .unwrap_or_else(|_| normalize_path(&file.to_string_lossy()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::process::Command;

    use async_doctor_core::Report;

    #[test]
    fn parse_requires_reason() {
        let src = "time.sleep(1)  # async-doctor: ignore[blocking-sleep]\n";
        let p = parse_file("a.py", src);
        assert!(p.suppressions.is_empty());
        assert_eq!(p.missing_reason.len(), 1);
        assert_eq!(p.missing_reason[0].rule, "blocking-sleep");

        let src = "time.sleep(1)  # async-doctor: ignore[blocking-sleep]: known sync boundary\n";
        let p = parse_file("a.py", src);
        assert_eq!(p.suppressions.len(), 1);
        assert_eq!(p.suppressions[0].reason, "known sync boundary");
    }

    #[test]
    fn hash_inside_string_is_not_a_comment() {
        let src = "x = \"# async-doctor: ignore[blocking-sleep]: fake\"\ntime.sleep(1)\n";
        let p = parse_file("a.py", src);
        assert!(p.suppressions.is_empty(), "{p:?}");
        assert!(p.missing_reason.is_empty());
    }

    #[test]
    fn hash_inside_single_quotes_ignored() {
        let src = "x = '# async-doctor: ignore[blocking-sleep]: fake'\n";
        assert!(parse_file("a.py", src).suppressions.is_empty());
    }

    #[test]
    fn hash_inside_multiline_triple_quotes_ignored() {
        let src = "doc = \"\"\"\n# async-doctor: ignore[blocking-sleep]: fake\n\"\"\"\n";
        let p = parse_file("a.py", src);
        assert!(p.suppressions.is_empty(), "{p:?}");
        assert!(p.missing_reason.is_empty(), "{p:?}");
    }

    #[test]
    fn inner_multiline_dict_ignore_does_not_bleed_to_earlier_entry() {
        let src = "d = {\n    'a': foo(),\n    'b': bar(),  # async-doctor: ignore[blocking-http]: bar only\n}\n";
        let p = parse_file("a.py", src);
        assert_eq!(p.suppressions.len(), 1);
        assert!(p.suppressions[0].matches_line(3));
        assert!(
            !p.suppressions[0].matches_line(2),
            "ignore on line 3 inside dict must not cover sibling line 2: {:?}",
            p.suppressions[0]
        );
    }

    #[test]
    fn files_to_scan_skips_gitignored_py() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();
        let git = |args: &[&str]| {
            assert!(
                Command::new("git")
                    .args(args)
                    .current_dir(root)
                    .env("GIT_AUTHOR_NAME", "t")
                    .env("GIT_AUTHOR_EMAIL", "t@t")
                    .env("GIT_COMMITTER_NAME", "t")
                    .env("GIT_COMMITTER_EMAIL", "t@t")
                    .status()
                    .unwrap()
                    .success()
            );
        };
        git(&["init", "-b", "main"]);
        fs::write(root.join(".gitignore"), "ignored/\n").unwrap();
        fs::write(root.join("app.py"), "async def h():\n    return 1\n").unwrap();
        fs::create_dir_all(root.join("ignored")).unwrap();
        fs::write(root.join("ignored/x.py"), "x = 1\n").unwrap();
        git(&["add", ".gitignore", "app.py"]);
        git(&["commit", "-m", "init"]);

        let paths = files_to_scan(&Report::empty_complete(), root, None).unwrap();
        assert_eq!(paths.len(), 1, "{paths:?}");
        assert!(
            paths[0].ends_with("app.py"),
            "expected only app.py, got {paths:?}"
        );
    }
}
