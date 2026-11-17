# Withdraw swallowed-cancelled-error advice

Status: accepted

## Problem

`swallowed-cancelled-error` advice flagged many legitimate or low-signal
`except asyncio.CancelledError` / bare-`except` / `except BaseException`
patterns in real projects. Advice is non-blocking for the exit code, but noisy
advice still fails the product bar: false positives are correctness bugs, and
agents treat advice as actionable.

## Decision

Withdraw `swallowed-cancelled-error` from normal scans and from the active rule
catalog starting in 0.1.6:

- Extraction no longer seeds `CalleeRef::External` edges with the
  `swallowed-cancelled-error:` prefix.
- Propagation no longer emits advice for that id.
- `RULE_DOCS` / `async-doctor explain` no longer treat it as a known rule.
- `docs/rules.md` keeps a short `#swallowed-cancelled-error` withdrawn note so
  old links remain meaningful.

`offload-stateful-session` and `useless-local-async-lock` stay unchanged.

## Alternatives considered

- **Keep emitting and document ignore guidance.** Rejected: the only supported
  escape hatch is inline `# async-doctor: ignore[...]`, and asking users to
  suppress a noisy built-in advice rule fights zero-config.
- **Keep a dormant RuleDoc / explain entry while silencing emission.** Rejected:
  `explain` would still present the id as an active rule, which conflicts with
  "not selectable as an active rule".
- **Special-case unused suppressions for the withdrawn id.** Rejected: no
  existing withdrawn-rule suppression registry; silent acceptance of stale
  ignores would hide cleanup work. Call out the unused-suppression consequence
  instead.

## Consequences

- Scans stop reporting the rule; cold/warm caches invalidate via the tool
  version bump (`0.1.5` → `0.1.6`).
- `async-doctor explain swallowed-cancelled-error` exits 2 (`unknown rule`).
- Existing `# async-doctor: ignore[swallowed-cancelled-error]: <reason>` comments
  become unused suppressions and fail the run (exit 1 unless `--fail-on none`)
  until removed.
- The rule may return only with a precision design that meets the false-positive
  bar; record that design in a new decision rather than silently re-enabling.
