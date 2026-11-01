# M1 — Call graph + propagation

## Purpose

Build a typed call graph, propagate stdlib sinks to `async def` entries with
shortest evidence chains, cut offload boundaries, and emit JSON/text with
prove-only conditions.

## Progress

- [x] Type-resolved call graph in the adapter (`SemanticModel` + definitions / `HasType`).
- [x] Blocking propagation from stdlib sinks up the reverse call graph; BFS shortest chain.
- [x] Offload cuts: `asyncio.to_thread`, `run_in_executor`, `anyio.to_thread.run_sync`, `starlette.concurrency.run_in_threadpool`, `ThreadPoolExecutor.submit/map`, plus callable refs / `partial` / lambdas. Eager `to_thread(f())` still reports.
- [x] Entry labels inferred; sync FastAPI/Starlette-style route handlers are not entries.
- [x] Unresolved dynamic dispatch → `candidates`; metadata IO → candidates; findings only when provable.
- [x] Condition tags prove-only; dead-branch pruning for unreassigned module constants; generator create-without-iterate does not propagate.
- [x] Zero-config non-production skip; merge same-entry `open`+`read`/`write`; stable `id` without line/column.
- [x] Async-twin names in `fix.hint` (advice tier came later).

## Surprises & Discoveries

- Parameter guards in callees with non-literal caller args demote to candidates; literal falsy args prune.
- Entry-local param guards stay findings (`async-branch`).
- Lambda bodies stay deferred unless invoked; offload-passed lambdas are cut.

## Decision log

- Decision: reporting tiers and exit codes / Date: 2026-10-30 / Link: [../../decisions/2026-10-30-reporting-contract.md](../../decisions/2026-10-30-reporting-contract.md).

## Validation

- Stdlib rule fixtures and `cargo test --workspace` green on the M1 merge.

## Outcomes & Retrospective

M1 delivered the prove-only core. Third-party follow and package cache deferred to M2.
