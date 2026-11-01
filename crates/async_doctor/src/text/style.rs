//! TTY-aware ANSI styling via `anstyle` and `anstream`.
//!
//! When color is disabled (piped stdout without `CLICOLOR_FORCE`/`FORCE_COLOR`, or
//! when `NO_COLOR` is set), every [`anstyle::Style`] in [`Palette`] is empty so
//! `{style}text{style:#}` emits zero escape bytes.

use anstream::{AutoStream, ColorChoice};
use anstyle::{AnsiColor, Style};

#[derive(Debug, Clone, Copy)]
pub struct Palette {
    pub bold: Style,
    pub rule_finding: Style,
    pub rule_candidate: Style,
    pub rule_advice: Style,
    pub ok: Style,
    pub fix: Style,
    pub choke: Style,
    pub dim: Style,
}

impl Palette {
    /// Detect terminal color support for stdout (`NO_COLOR` > `CLICOLOR_FORCE`/`FORCE_COLOR` > TTY).
    pub fn stdout() -> Self {
        if std::env::var_os("NO_COLOR").is_some() {
            return Self::plain();
        }
        if std::env::var("FORCE_COLOR").is_ok_and(|v| !v.is_empty() && v != "0") {
            return Self::ansi();
        }
        if AutoStream::choice(&std::io::stdout()) != ColorChoice::Never {
            Self::ansi()
        } else {
            Self::plain()
        }
    }

    pub const fn plain() -> Self {
        Self {
            bold: Style::new(),
            rule_finding: Style::new(),
            rule_candidate: Style::new(),
            rule_advice: Style::new(),
            ok: Style::new(),
            fix: Style::new(),
            choke: Style::new(),
            dim: Style::new(),
        }
    }

    pub const fn ansi() -> Self {
        Self {
            bold: Style::new().bold(),
            rule_finding: Style::new()
                .bold()
                .fg_color(Some(anstyle::Color::Ansi(AnsiColor::Red))),
            rule_candidate: Style::new()
                .bold()
                .fg_color(Some(anstyle::Color::Ansi(AnsiColor::Yellow))),
            rule_advice: Style::new()
                .bold()
                .fg_color(Some(anstyle::Color::Ansi(AnsiColor::Cyan))),
            ok: Style::new()
                .bold()
                .fg_color(Some(anstyle::Color::Ansi(AnsiColor::Green))),
            fix: Style::new().fg_color(Some(anstyle::Color::Ansi(AnsiColor::Green))),
            choke: Style::new()
                .bold()
                .fg_color(Some(anstyle::Color::Ansi(AnsiColor::Yellow))),
            dim: Style::new().dimmed(),
        }
    }
}
