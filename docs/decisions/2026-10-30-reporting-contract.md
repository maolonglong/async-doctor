# Findings, candidates, exit codes, and JSON schema

Status: accepted

## Problem

Agents need a stable machine-readable contract: what fails CI, what is
uncertain, and how to explain a site. Mixing uncertain hits into failing
findings trains agents to ignore the tool.

## Decision

- **Tiers:** prove-only `findings[]`; uncertain cases go to `candidates[]`
  (with `reason`); never guess a finding by name alone.
- **Exit codes:** `0` no findings (and no unused suppressions); `1` findings
  present (even if analysis is incomplete); `2` tool error only. Incomplete
  analysis sets `complete=false` plus `skipped` / stderr warning — not exit 2.
  Candidates (and later advice) never affect exit code.
- **JSON (`schemaVersion` 2):** always emit top-level arrays even when empty;
  include `environment` (interpreter/venv/site-packages + advisory `note`) and
  `locationLessCandidates`; `why --path` is relative to the scan cwd, never an
  absolute machine path. Default human text; `--format json` / `sarif` for
  agents and code scanning.
- **Entry labels** (`route` / `lifecycle` / `task` / `other`) are auto-inferred
  and affect ordering/presentation only.
- **Async twin names** appear in `fix.hint` for findings; they are not a
  separate finding tier (see also the advice exception in
  [2026-10-31-advice-tier.md](2026-10-31-advice-tier.md)).

## Alternatives considered

- **Fail the exit code on candidates or incomplete analysis.** Rejected: would
  punish missing types / timeouts and push users to disable the tool.
- **Async-twin hits as their own finding rule.** Rejected for the base case:
  twins are fix guidance; the later advice tier covers already-cut offloads
  without double-reporting `blocking-*`.

## Consequences

Every false positive in `findings` is a correctness bug with a regression
fixture. Schema bumps invalidate scan/package caches. Topic reference:
[../json-contract.md](../json-contract.md).
