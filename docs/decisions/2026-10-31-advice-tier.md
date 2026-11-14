# Non-blocking `advice[]` tier for async anti-patterns

Status: superseded by [Evidence-bounded repair guidance](2026-11-14-evidence-bounded-repair-guidance.md)

The replacement revises repair interpretation only and explicitly retains the
advice tier, category values, resolution gates, exclusions, and exit-code policy.

## Problem

Some async anti-patterns are real (for example wrapping a sync HTTP client in
`asyncio.to_thread` when a same-package async twin exists) but must not fail CI
or double-report with `blocking-*` findings that the offload already cuts.

## Decision

Emit a separate top-level `advice[]` array for non-blocking diagnostics.
`offload-has-async-twin` fires only at offload sites that are already cut, and
only when every reached sink has a high-confidence same-package async twin
that resolves (with its `needs`) in the scanned venv. Advice uses category
`performance` | `correctness`, maps to SARIF `note`, appears in a text Advice
section, and never changes the exit code. Structured twins live in the
`async_twin` table on sink families. This is an explicit exception to the
earlier “async twin is only a fix-hint string” rule in
[2026-10-30-reporting-contract.md](2026-10-30-reporting-contract.md).

Shipped with related error findings `nested-event-loop` and
`offload-coroutine` (those remain findings). See
https://github.com/maolonglong/async-doctor/pull/30.

## Alternatives considered

- **Report offload-has-async-twin as a finding.** Rejected: fails CI for
  already-safe offloads and fights the “false positives are worse than misses”
  bar.
- **Keep twins only inside `fix.hint` forever.** Rejected for cut offloads:
  there is no blocking finding left to hang the hint on, so the signal would
  disappear.

## Consequences

Advice suppressions reuse `# async-doctor: ignore[rule]: <reason>`. Graders
must not fold advice into findings. Rule catalogue:
[../rules.md](../rules.md).
