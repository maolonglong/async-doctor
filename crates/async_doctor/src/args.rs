//! Clap surface for the default scan and help subcommands.

use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

#[derive(Debug, Clone, Copy, ValueEnum, PartialEq, Eq)]
pub enum Format {
    Text,
    Json,
}

#[derive(Parser, Debug)]
#[command(
    name = "async-doctor",
    version,
    about = "Your agent blocks the event loop. This catches it.",
    long_about = "Detect sync/blocking IO reachable from Python async def code.\n\
                  Zero-config: point at a project directory and run.\n\n\
                  Exit codes: 0 = clean, 1 = findings, 2 = tool error.",
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
