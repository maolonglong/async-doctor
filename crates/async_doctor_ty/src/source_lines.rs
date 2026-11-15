//! Per-line lexical facts derived from `ruff_python_parser` tokens.
//!
//! Powers `#` pragma consumers (inline suppressions) with token-accurate
//! comment detection: `#` inside string literals — including nested same-quote
//! f-string literals under PEP 701 — never registers as a comment, while real
//! comments inside f-string replacement fields still do.

use ruff_python_ast::token::TokenKind;
use ruff_python_parser::{Mode, ParseOptions, parse_unchecked};
use ruff_text_size::Ranged;

/// Lexical facts about one source line. Mirrors the fields pragma consumers
/// need: trailing-comment detection, bracket depth, logical-statement starts,
/// closed bracket open-lines, and `@decorator` statement membership.
#[derive(Debug, Default, Clone)]
pub struct LineFacts {
    /// A real code token precedes the comment on this line.
    pub has_code_before_comment: bool,
    /// Line belongs to a `@decorator` statement, including continuation lines
    /// of a multi-line decorator.
    pub is_decorator_line: bool,
    /// Byte range of the comment text (after the `#`, before EOL/`\r`).
    pub comment_range: Option<(usize, usize)>,
    /// Unclosed `(` / `[` / `{` count at the comment.
    pub depth_at_comment: usize,
    /// Start line of the enclosing logical statement at the comment.
    pub stmt_start_at_comment: Option<u32>,
    /// Start lines of brackets that closed on this line.
    pub closed_open_lines: Vec<u32>,
}

/// Scan `source` once over `parse_unchecked(..).tokens()` and return per-line
/// facts, 1-indexed (`facts[1]` is line 1). Syntax errors still yield facts:
/// error-recovering token streams degrade gracefully rather than dropping
/// comment detection for the rest of the file.
pub fn scan_line_facts(source: &str) -> Vec<LineFacts> {
    // 1-based line lookup: `line_starts[i-1]` is the byte offset of line i.
    let mut line_starts = vec![0usize];
    line_starts.extend(source.match_indices('\n').map(|(i, _)| i + 1));
    let line_of = |off: usize| line_starts.partition_point(|&l| l <= off) as usize;
    let mut lines = vec![LineFacts::default(); line_starts.len() + 1];

    let parsed = parse_unchecked(source, ParseOptions::from(Mode::Module));
    let mut bracket_stack: Vec<(TokenKind, u32)> = Vec::new();
    let mut logical_start: Option<u32> = None;
    let mut at_logical_start = true;
    let mut decorator_active = false;

    for token in parsed.tokens().iter() {
        let kind = token.kind();
        let start = token.start().to_usize();
        let end = token.end().to_usize();
        match kind {
            TokenKind::Comment => {
                let mut end = end;
                if end > start + 1 && source.as_bytes().get(end - 1) == Some(&b'\r') {
                    end -= 1;
                }
                let line = line_of(start);
                let info = &mut lines[line];
                info.comment_range = Some((start + 1, end));
                info.depth_at_comment = bracket_stack.len();
                info.stmt_start_at_comment = logical_start;
                continue;
            }
            TokenKind::Newline => {
                logical_start = None;
                decorator_active = false;
                at_logical_start = true;
                continue;
            }
            // Continuation, layout, and sentinel tokens carry no line facts.
            TokenKind::NonLogicalNewline
            | TokenKind::Indent
            | TokenKind::Dedent
            | TokenKind::EndOfFile => continue,
            _ => {}
        }

        let line = line_of(start);
        let end_line = line_of(end.saturating_sub(1).max(start));
        lines[line].has_code_before_comment = true;
        if end_line > line {
            // A multi-line token (triple-quoted string, f-string middle) leaves
            // closing text on its tail line — that tail counts as code before
            // a trailing comment, and coverage reaches back to the start line
            // the way a closed bracket does.
            lines[end_line].has_code_before_comment = true;
            lines[end_line].closed_open_lines.push(line as u32);
        }
        if decorator_active {
            for info in lines.iter_mut().take(end_line + 1).skip(line) {
                info.is_decorator_line = true;
            }
        }
        if at_logical_start {
            at_logical_start = false;
            // An `@` opening a logical line at bracket depth 0 is a decorator;
            // `x @ y` matmul never starts a logical line.
            if kind == TokenKind::At && bracket_stack.is_empty() {
                decorator_active = true;
                lines[line].is_decorator_line = true;
            }
        }
        if logical_start.is_none() {
            logical_start = Some(line as u32);
        }
        match kind {
            TokenKind::Lpar | TokenKind::Lsqb | TokenKind::Lbrace => {
                bracket_stack.push((kind, line as u32));
            }
            TokenKind::Rpar | TokenKind::Rsqb | TokenKind::Rbrace => {
                let expected = match kind {
                    TokenKind::Rpar => TokenKind::Lpar,
                    TokenKind::Rsqb => TokenKind::Lsqb,
                    _ => TokenKind::Lbrace,
                };
                if let Some(pos) = bracket_stack.iter().rposition(|(k, _)| *k == expected) {
                    let (_, open_line) = bracket_stack[pos];
                    bracket_stack.truncate(pos);
                    lines[end_line].closed_open_lines.push(open_line);
                }
            }
            TokenKind::Semi if bracket_stack.is_empty() => {
                logical_start = None;
                decorator_active = false;
                at_logical_start = true;
            }
            _ => {}
        }
    }
    lines
}
