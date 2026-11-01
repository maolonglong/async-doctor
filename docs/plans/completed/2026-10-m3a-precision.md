# M3a precision / recall notes

## Purpose

Close M3a corpus gaps (sync redis awaited, context enter, consume-only sinks,
eager offload) without regressing the FP=0 negatives.

## Progress

- [x] 2026-10: Sync redis awaited / attribute / gather / watch — fixed.
- [x] 2026-10: `with Engine.begin()` WithEnter recording — fixed.
- [x] 2026-10: `PubSub.listen` consume-only — fixed.
- [x] 2026-10: Eager `to_thread(f())` / `run_in_executor(None, f())` — fixed.
- [x] 2026-10: Documented out-of-scope `unsafe_async_mutation`.

## Surprises & Discoveries

Philosophy (react-doctor-aligned): zero config, high-signal findings, every
finding actionable. **False positives are worse than misses.**

### Corpus status (fixtures under `fixtures/m3a_*`)

| Case | Verdict | Notes |
| --- | --- | --- |
| Sync redis awaited (`await r.get(...)`) | **Fixed** | Sync body runs before `await`; no longer blanket-skipped on `is_awaited`. Async counterparts still never fire. |
| Sync redis via attribute / gather / watch | **Fixed** | Covered by receiver-typed Externals + family matchers. |
| `begin_entered` (`with Engine.begin()`) | **Fixed** | Context `Call` is recorded as `WithEnter` so `with_only` sinks fire. Bare `engine.begin()` (not entered) stays quiet. |
| `listen_consumed` / `listen_comprehension` | **Fixed** | `redis.PubSub.listen` is `consume_only` — fires for `for` / comprehension / `list`/`next`, not bare `listen()`. |
| `unsafe_async_mutation` | **Out of scope** | Shared mutable state without a blocking sink is a concurrency hazard, not event-loop blocking IO. async-doctor does not report it. |
| Eager `to_thread(f())` / `run_in_executor(None, f())` | **Fixed** | Finding + fix hint: pass the callable rather than calling it. |

### Negatives that must stay clean (FP = 0)

- `redis.asyncio` client / pipeline / `await pipe.execute()`
- Sync `Pipeline` buffering (`set`/`get`) without `execute()`
- Bare `engine.begin()` and bare `pubsub.listen()` (not entered / not consumed)
- `asyncio.to_thread(work)` / `run_in_executor(None, work)` with a callable ref
- Awaited async HTTP clients (`httpx.AsyncClient`, `elasticsearch.AsyncElasticsearch`)
- Subclasses / wrappers of async clients (structural `async def` callee — not name rules)
- `AsyncOpenAI` stream / `openai.lib._parsing._completions` helpers (not sync `OpenAI`)

## Decision log

- Decision: sync body under `await` still seeds when the callee is a known sync sink / Date: 2026-10 / Rationale: the sync body runs on the loop before await.
- Decision: shared mutable state without a blocking sink stays out of scope / Date: 2026-10 / Rationale: product is event-loop blocking IO, not general concurrency hazards.

## Validation

- Fixtures under `fixtures/m3a_*` and `cargo test -p async_doctor_ty --test` / crate tests for m3a precision pass on main.

## Outcomes & Retrospective

M3a corpus gaps listed above closed. Higher-order / narrowing / in-memory IO
tracked in [2026-10-m3a-higher-order.md](2026-10-m3a-higher-order.md). Remaining
recall misses live in [../../quality.md](../../quality.md).
