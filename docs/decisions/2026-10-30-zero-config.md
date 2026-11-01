# Zero-config product surface

Status: accepted

## Problem

Agent and CI users will not maintain a project-specific async-doctor config.
Any required config file becomes a adoption cliff and a source of divergent
behaviour across repos.

## Decision

No config file. Entries (`route` / `lifecycle` / `task` / `other`), builtin
rules, offload boundaries, and non-production path skips are built-in or
inferred from the project and environment. The only escape hatch is an inline
comment: `# async-doctor: ignore[<rule-id>]: <reason>` (reason required).
CLI flags may filter reporting (`--changed`, `--baseline`, `--show-unresolved`,
`--include-non-production`) but do not replace discovery rules.

## Alternatives considered

- **TOML/YAML project config for entries, sinks, and ignores.** Rejected: fights
  the react-doctor-like zero-config pitch and creates per-repo dialect.
- **Config-only ignores without inline comments.** Rejected: agents and local
  edits need a file-local suppress that travels with the code.

## Consequences

New behaviour must be inferred or flagged on the CLI, not configured. Rule
and sink growth happens in `async_doctor_rules`, not user files. Precision
work prefers better inference over escape hatches.
