//! Clap surface for the default scan and help subcommands.

use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

#[derive(Debug, Clone, Copy, ValueEnum, PartialEq, Eq)]
pub enum Format {
    Text,
    Json,
    /// SARIF 2.1.0 (GitHub code scanning).
    Sarif,
}

#[derive(Parser, Debug)]
#[command(
    name = "async-doctor",
    version,
    about = "Your agent blocks the event loop. This catches it.",
    long_about = "Detect sync/blocking IO reachable from Python async def code.\n\
                  Zero-config: point at a project directory and run.\n\n\
                  Exit codes:\n\
                    0 = clean (no findings / no new findings vs baseline)\n\
                    1 = findings present (or unused suppressions)\n\
                    2 = tool error\n\
                  Incomplete analysis sets complete=false; it does not use exit 2.",
    args_conflicts_with_subcommands = true,
    subcommand_negates_reqs = true
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,

    /// Project directory or a single `.py` file to scan (default: current directory).
    #[arg(default_value = ".")]
    pub path: PathBuf,

    /// Output format.
    #[arg(long, value_enum, default_value_t = Format::Text)]
    pub format: Format,

    /// Include entries under tests/, docs/, examples/, scripts/, and hidden dirs.
    #[arg(long)]
    pub include_non_production: bool,

    /// Include `reason=unresolved` candidates in the report (hidden by default).
    #[arg(long)]
    pub show_unresolved: bool,

    /// Print every finding with its full chain (default human output is grouped).
    #[arg(long)]
    pub expand: bool,

    /// Report only findings whose chain touches files changed vs GIT_REF.
    ///
    /// Use `--changed` for the merge-base with the default branch plus
    /// uncommitted changes, or `--changed=GIT_REF` for an explicit ref.
    /// (`=` is required so the project path is not swallowed.) Analysis still
    /// covers the whole project; only reporting is filtered.
    ///
    /// Base detection order: `GITHUB_BASE_REF`, branch upstream, `origin/HEAD`,
    /// `origin/main|master`, local `main|master`. Outside a git repo — or when
    /// no base can be resolved (typical shallow detached CI checkout) — prints
    /// a note and falls back to the full report (never silently diffs HEAD).
    #[arg(
        long,
        num_args = 0..=1,
        default_missing_value = "AUTO",
        require_equals = true,
        value_name = "GIT_REF"
    )]
    pub changed: Option<String>,

    /// Suppress findings whose stable ids appear in this baseline file.
    /// Exit 1 only for findings not in the baseline (new findings).
    #[arg(long, value_name = "FILE")]
    pub baseline: Option<PathBuf>,

    /// Write current findings' stable ids to FILE (after suppressions; before
    /// `--baseline` filtering). Incompatible with `--changed` (would write a
    /// partial set).
    #[arg(long, value_name = "FILE")]
    pub write_baseline: Option<PathBuf>,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Print long-form help for a rule id (e.g. blocking-sleep).
    Explain {
        /// Rule id or docs anchor (`blocking-sleep` or `#blocking-sleep`).
        rule: String,
    },
    /// Explain findings that touch a file:line site (re-scans the project).
    Why {
        /// Location as `file:line` (matches the `why` field on findings).
        location: String,
        /// Project directory or `.py` file to scan (default: current directory).
        #[arg(long, default_value = ".")]
        path: PathBuf,
        /// Include non-production entries while locating the site.
        #[arg(long)]
        include_non_production: bool,
        /// Include unresolved candidates while locating the site.
        #[arg(long)]
        show_unresolved: bool,
    },
}
