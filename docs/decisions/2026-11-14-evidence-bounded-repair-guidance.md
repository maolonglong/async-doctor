# Separate diagnostic evidence from prescribed repairs

Status: accepted

## Problem

A proven blocking call does not establish whether thread offload, async client
migration, a new dependency, or a different transaction boundary fits the
application. Prescribing one can remove a diagnostic while breaking callers,
resource ownership, cancellation, or business semantics. Symbol availability
and similar function names do not prove equivalent behavior or an optimal edit.

## Decision

This revises repair guidance in the
[reporting contract](2026-10-30-reporting-contract.md) and
[advice-tier decision](2026-10-31-advice-tier.md). Their diagnostic tiers,
category values, grading, identity, grouping, and exit-code policies remain
unchanged; the current field contract is [../json-contract.md](../json-contract.md).

- Preserve schema 2 and every diagnostic's `fix` object, string `hint`, and
  stable `fixGroupId`. An empty hint means no recommended edit, not an absent
  diagnosis or a placeholder for an agent to fill.
- Builtin blocking sinks, unresolved candidates, and context-dependent async
  anti-patterns prescribe no offload, migration, scheduling, or ownership edit.
  Remove unsupported recommendations at their source, including name-only
  local async-twin inference, rather than filtering their final output.
- Only `sync-with-async-cm` and `sync-for-async-iterable` retain a correction to
  the supported async-only protocol on the same object, where async syntax is
  legal. This does not authorize a containing API conversion or claim a
  globally optimal repair.
- Keep `offload-has-async-twin` and its existing resolution gates and
  exclusions. Report the resolved same-package APIs in chain evidence, with
  no repair hint. Its legacy `performance` category is not a measurement or a
  claim that migration is suitable.
- Text, Markdown, SARIF, `explain`, and `why` keep diagnostic explanations
  independent of repair text and omit empty repair sections. Eager argument
  execution belongs in chain evidence, not a thread-rewrite instruction.
- Site and choke-point counts describe observed reachability, not guaranteed
  fixes. Sync-caller counts cover the analyzed graph, not unknown external or
  dynamic callers.
- The product skill requires inspection of intent, callers, ownership,
  transactions, cancellation, and installed library contracts before a repair.
  When suitability is unproven, retain the diagnosis and report the missing
  evidence instead of guessing an edit or suppression.

## Alternatives considered

- **Always recommend offload or a resolved async API.** Rejected: either can
  change ownership, lifetime, transaction, scheduling, or caller contracts that
  the scan does not establish.
- **Promise a globally optimal fix or add a confidence score.** Rejected: the
  scan cannot verify that promise; another score does not supply the missing
  application context.
- **Remove all repair text or introduce a new optional JSON shape.** Rejected:
  proven same-object protocol mismatches allow a bounded local correction,
  while schema 2 already represents an absent recommendation with a string.
- **Drop async-twin advice.** Rejected: resolved API availability remains a
  useful fact even when it does not justify a migration.

## Consequences

Diagnoses remain actionable through locations, evidence chains, and rule
explanations without implying a preferred rewrite. Consumers must handle an
empty `fix.hint`; grouping ids do not prove a shared repair. A clean rescan
alone does not verify a proposed application change.

The tool does not guarantee that an agent follows the skill. Agent-behavior
evaluations require separate approval; corpus audits verify diagnostic
preservation and output contracts, not optimal application repairs.
