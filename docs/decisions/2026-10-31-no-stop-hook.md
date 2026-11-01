# Ship the skill install path; no editor stop hook

Status: accepted

## Problem

Early agent-integration drafts included Claude/Cursor stop-hook wiring under
`integrations/`. Stop hooks couple the tool to editor lifecycle events, add
packaging surface, and diverge from the react-doctor distribution model users
already know.

## Decision

Do not ship a Claude or Cursor stop hook. Agent integration is the CLI plus a
thin skill at `skills/async-doctor/SKILL.md` discoverable via
`npx skills add maolonglong/async-doctor` once the repo is public. README
documents the skill install and SARIF upload only. See
https://github.com/maolonglong/async-doctor/pull/29.

## Alternatives considered

- **Keep `integrations/stop-hook/` and editor hooks.** Rejected: maintenance
  and auth surface without clear adoption upside versus `npx skills add`.
- **MCP-only integration before a skill.** Deferred: MCP is post-M4 if needed;
  the skill covers agent loops with `--changed` today.

## Consequences

Do not reintroduce stop-hook packages without a superseding decision. The
product skill under `skills/` stays separate from repo-dev skills under
`.agents/skills/`.
