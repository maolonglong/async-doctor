# `concurrent.futures.Future.result` / `.exception` stay finding-tier

Status: accepted

## Problem

`Future.result()` and `Future.exception()` block the calling thread until the
future completes or the timeout expires. Reached from `async def` code on the
event loop, a pending wait is a real blocking operation — but static analysis
can also see many call sites where the future is already done. The choice is
whether these sinks stay `findings`, move to `candidates`, or are suppressed
unless locally proven pending.

## Decision

`concurrent.futures.Future.result` and `.exception` remain finding-tier unless
completion is proven locally. Proven cases: typed `.done()` guards on the same
receiver and nonpositive literal timeouts (poll-only calls). A pending wait
blocks the event loop; local analysis cannot prove cross-coroutine
coordination, and a global downgrade would hide true waits.

The two Starlette `testclient.py` sites that wait on already-completed futures
are a documented precision exception, not grounds for downgrading the sink.
Users hit by the same pattern suppress with inline
`# async-doctor: ignore[blocking-lock]: <reason>`.

See [docs/quality.md](../quality.md) (Future completion proofs row),
[docs/rules.md](../rules.md#blocking-lock), and
[docs/notes/2026-11-14-agent-repair-evaluation.md](../notes/2026-11-14-agent-repair-evaluation.md).

## Alternatives considered

- Downgrade all future waits to `candidates`: hides true event-loop waits
  behind a flood of advisory entries; rejected.
- Suppress whenever any `.done()`-adjacent proof shape appears nearby: the
  proofs that exist today are deliberately local and structural
  (same-receiver, typed guards); widening to arbitrary call sites or alias
  flows reintroduces unproven-wait false negatives.

## Consequences

Call sites that coordinate futures across coroutines without local proof keep
a finding and need an inline suppression. If analysis gains reliable
cross-coroutine completion proofs, the proven-local set can grow without
changing the tier.
