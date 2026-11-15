//! Pragma parser for inline suppressions over ruff-token line facts.
//!
//! Comment detection comes from `async_doctor_ty::source_lines` (ruff tokens),
//! so `#` inside string literals — including PEP 701 nested same-quote
//! f-strings — is never a comment. This module keeps bracket/logical-statement
//! metadata, `@decorator` spans, stacked `#` pragmas (`# type: ignore[...]`,
//! `# noqa: ...`), and comma-separated rule lists
//! (`# async-doctor: ignore[rule-a, rule-b]: <reason>`).

use std::collections::HashSet;

use async_doctor_ty::source_lines::{LineFacts, scan_line_facts};

use crate::paths::normalize_path;

use super::{MissingReasonIgnore, Suppression};

#[derive(Debug, Default)]
pub(super) struct ParseResult {
    pub suppressions: Vec<Suppression>,
    pub missing_reason: Vec<MissingReasonIgnore>,
}

pub(super) fn parse_file(file: &str, source: &str) -> ParseResult {
    let lines = scan_line_facts(source);
    let norm_file = normalize_path(file);
    let mut out = ParseResult::default();

    for idx in 1..lines.len() {
        let line_no = idx as u32;
        let info = &lines[idx];
        let Some((start, end)) = info.comment_range else {
            continue;
        };
        let raw_comment = &source[start..end];
        let standalone = !info.has_code_before_comment;
        let decorated_def_line = if standalone {
            find_decorated_target_line(&lines, idx)
        } else {
            None
        };
        let covered_lines = compute_covered_lines(info, line_no);

        let mut seen_rules_on_line: HashSet<String> = HashSet::new();
        for segment in pragma_segments(raw_comment) {
            match parse_ignore_comment(segment.trim()) {
                IgnoreParse::Suppression { rules, reason } => {
                    for rule in rules {
                        if seen_rules_on_line.insert(rule.clone()) {
                            out.suppressions.push(Suppression {
                                file: norm_file.clone(),
                                line: line_no,
                                rule,
                                reason: reason.clone(),
                                standalone,
                                decorated_def_line,
                                covered_lines: covered_lines.clone(),
                            });
                        }
                    }
                }
                IgnoreParse::MissingReason { rules } => {
                    for rule in rules {
                        out.missing_reason.push(MissingReasonIgnore {
                            file: norm_file.clone(),
                            line: line_no,
                            rule,
                        });
                    }
                }
                IgnoreParse::None => {}
            }
        }
    }
    out
}

fn compute_covered_lines(info: &LineFacts, line_no: u32) -> Vec<u32> {
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
    covered_lines
}

/// Split a raw comment tail (after the leading `#`) into stacked pragma segments.
///
/// Boundaries occur at `<ws>#<ws>` (e.g. `# type: ignore[x]  # async-doctor: ...`)
/// or `<ws>#<known-pragma>` (e.g. `#noqa`, `#async-doctor:`), preserving issue
/// references like `see issue #42` inside `<reason>`.
fn pragma_segments(raw_comment: &str) -> Vec<&str> {
    let bytes = raw_comment.as_bytes();
    let mut cuts = vec![0usize];
    let mut i = 1;
    while i < bytes.len() {
        if bytes[i] == b'#' && matches!(bytes[i - 1], b' ' | b'\t') {
            let after_hash = &raw_comment[i + 1..];
            let has_ws_after = after_hash
                .as_bytes()
                .first()
                .is_some_and(|b| matches!(*b, b' ' | b'\t'));
            let trimmed = after_hash.trim_start_matches('#').trim_start();
            if has_ws_after || starts_with_known_pragma(trimmed) {
                cuts.push(i + 1);
            }
        }
        i += 1;
    }

    let mut segments = Vec::with_capacity(cuts.len());
    for (idx, &start) in cuts.iter().enumerate() {
        let end = cuts
            .get(idx + 1)
            .map(|next| next.saturating_sub(1))
            .unwrap_or(raw_comment.len());
        let seg = raw_comment[start..end].trim_start_matches('#').trim();
        if !seg.is_empty() {
            segments.push(seg);
        }
    }
    segments
}

fn starts_with_known_pragma(s: &str) -> bool {
    const PREFIXES: &[&str] = &[
        "async-doctor:",
        "async-doctor :",
        "type:",
        "type :",
        "noqa",
        "ruff:",
        "pyright:",
        "pylint:",
        "pragma:",
        "fmt:",
        "nosec",
    ];
    PREFIXES
        .iter()
        .any(|p| s.len() >= p.len() && s[..p.len()].eq_ignore_ascii_case(p))
}

/// If standalone comment at `comment_line` is immediately followed by one or
/// more `@decorator` lines (including multi-line `@dec(...)` spans), return the
/// line number of the `def` / `async def` immediately after the decorator block.
fn find_decorated_target_line(lines: &[LineFacts], comment_line: usize) -> Option<u32> {
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

enum IgnoreParse {
    Suppression { rules: Vec<String>, reason: String },
    MissingReason { rules: Vec<String> },
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
    let rules: Vec<String> = rest[1..end]
        .split(',')
        .map(str::trim)
        .filter(|r| !r.is_empty())
        .map(ToString::to_string)
        .collect();
    if rules.is_empty() {
        return IgnoreParse::None;
    }
    let after = rest[end + 1..].trim_start();
    match after.strip_prefix(':') {
        Some(reason) => {
            let reason = reason.trim();
            if reason.is_empty() {
                IgnoreParse::MissingReason { rules }
            } else {
                IgnoreParse::Suppression {
                    rules,
                    reason: reason.to_string(),
                }
            }
        }
        None => IgnoreParse::MissingReason { rules },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn hash_inside_strings_ignored() {
        for src in [
            "x = \"# async-doctor: ignore[blocking-sleep]: fake\"\ntime.sleep(1)\n",
            "x = '# async-doctor: ignore[blocking-sleep]: fake'\n",
            "doc = \"\"\"\n# async-doctor: ignore[blocking-sleep]: fake\n\"\"\"\n",
        ] {
            let p = parse_file("a.py", src);
            assert!(p.suppressions.is_empty(), "{p:?}");
            assert!(p.missing_reason.is_empty(), "{p:?}");
        }
    }

    #[test]
    fn inner_multiline_dict_ignore_does_not_bleed_to_earlier_entry() {
        let src = "d = {\n    'a': foo(),\n    'b': bar(),  # async-doctor: ignore[blocking-http]: bar only\n}\n";
        let p = parse_file("a.py", src);
        assert_eq!(p.suppressions.len(), 1);
        assert!(p.suppressions[0].matches_line(3));
        assert!(!p.suppressions[0].matches_line(2));
        assert!(!p.suppressions[0].matches_line(4));
    }

    #[test]
    fn trailing_ignore_does_not_match_next_line_standalone_does() {
        let src = concat!(
            "time.sleep(0.01)  # async-doctor: ignore[blocking-sleep]: ok\n",
            "time.sleep(10)\n",
            "# async-doctor: ignore[blocking-sleep]: standalone\n",
            "time.sleep(0.02)\n",
        );
        let p = parse_file("a.py", src);
        assert_eq!(p.suppressions.len(), 2);
        assert!(p.suppressions[0].matches_line(1));
        assert!(!p.suppressions[0].matches_line(2));
        assert!(p.suppressions[1].matches_line(3));
        assert!(p.suppressions[1].matches_line(4));
    }

    #[test]
    fn nested_same_quote_fstring_hash_is_not_a_comment() {
        // PEP 701: `{"..."}` nested inside `f"..."` is a string literal — the
        // `#` inside it must not suppress anything or register a suppression.
        let src =
            "x = f\"{'#'} {\"# async-doctor: ignore[blocking-sleep]: fake\"}\"\ntime.sleep(1)\n";
        let p = parse_file("a.py", src);
        assert!(p.suppressions.is_empty(), "{p:?}");
        assert!(p.missing_reason.is_empty(), "{p:?}");
    }

    #[test]
    fn triple_fstring_replacement_field_hash_is_not_a_comment() {
        let src =
            "x = f\"\"\"{\"# async-doctor: ignore[blocking-sleep]: fake\"}\"\"\"\ntime.sleep(1)\n";
        let p = parse_file("a.py", src);
        assert!(p.suppressions.is_empty(), "{p:?}");
    }

    #[test]
    fn tstring_and_format_spec_hash_are_not_comments() {
        // `#` inside a format spec / nested replacement field is string text.
        for src in [
            "x = f\"{1:#x}\"\ntime.sleep(1)\n",
            "x = t\"{v!r:>{'#'}}\"\ntime.sleep(1)\n",
            "x = f\"{v:{'#'}}\"\ntime.sleep(1)\n",
        ] {
            let p = parse_file("a.py", src);
            assert!(p.suppressions.is_empty(), "{src}: {p:?}");
        }
    }

    #[test]
    fn trailing_comment_after_nested_fstring_still_suppresses() {
        let src = "x = f\"{'#'} {\"nested\"}\"; time.sleep(1)  # async-doctor: ignore[blocking-sleep]: real\n";
        let p = parse_file("a.py", src);
        assert_eq!(p.suppressions.len(), 1, "{p:?}");
        assert!(!p.suppressions[0].standalone);
        assert!(p.suppressions[0].matches_line(1));
        assert_eq!(p.suppressions[0].reason, "real");
    }

    #[test]
    fn comment_on_triple_string_tail_is_trailing_not_standalone() {
        // `abc"""  # pragma` on the closing line of a triple-quoted string
        // trails that statement — it must not cover the following line.
        let src =
            "x = \"\"\"ab\ncd\"\"\"  # async-doctor: ignore[blocking-sleep]: r\ntime.sleep(2)\n";
        let p = parse_file("a.py", src);
        assert_eq!(p.suppressions.len(), 1, "{p:?}");
        assert!(!p.suppressions[0].standalone, "{p:?}");
        assert!(p.suppressions[0].matches_line(1), "{p:?}");
        assert!(p.suppressions[0].matches_line(2), "{p:?}");
        assert!(!p.suppressions[0].matches_line(3), "{p:?}");
    }

    #[test]
    fn stacked_pragmas_and_comma_separated_rules_parse() {
        let src = "x()  # type: ignore[arg-type]  # async-doctor: ignore[rule-a, rule-b]: see #7  # noqa: E501\n";
        let p = parse_file("a.py", src);
        assert_eq!(p.suppressions.len(), 2);
        assert_eq!(p.suppressions[0].rule, "rule-a");
        assert_eq!(p.suppressions[1].rule, "rule-b");
        assert_eq!(p.suppressions[0].reason, "see #7");
        assert_eq!(p.suppressions[1].reason, "see #7");
    }
}
