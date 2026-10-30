# M3a precision / recall notes

Philosophy (react-doctor-aligned): zero config, high-signal findings, every
finding actionable. **False positives are worse than misses.**

## Corpus status (fixtures under `fixtures/m3a_*`)

| Case | Verdict | Notes |
| --- | --- | --- |
| Sync redis awaited (`await r.get(...)`) | **Fixed** | Sync body runs before `await`; no longer blanket-skipped on `is_awaited`. Async counterparts still never fire. |
| Sync redis via attribute / gather / watch | **Fixed** | Covered by receiver-typed Externals + family matchers. |
| `begin_entered` (`with Engine.begin()`) | **Fixed** | Context `Call` is recorded as `WithEnter` so `with_only` sinks fire. Bare `engine.begin()` (not entered) stays quiet. |
| `listen_consumed` / `listen_comprehension` | **Fixed** | `redis.PubSub.listen` is `consume_only` — fires for `for` / comprehension / `list`/`next`, not bare `listen()`. |
| `unsafe_async_mutation` | **Out of scope** | Shared mutable state without a blocking sink is a concurrency hazard, not event-loop blocking IO. async-doctor does not report it. |
| Eager `to_thread(f())` / `run_in_executor(None, f())` | **Fixed** | Finding + fix hint: pass the callable rather than calling it. |

## Negatives that must stay clean (FP = 0)

- `redis.asyncio` client / pipeline / `await pipe.execute()`
- Sync `Pipeline` buffering (`set`/`get`) without `execute()`
- Bare `engine.begin()` and bare `pubsub.listen()` (not entered / not consumed)
- `asyncio.to_thread(work)` / `run_in_executor(None, work)` with a callable ref

## Measurement notes (M3a agent)

The private 49-positive / 29-negative internal eval corpus is not checked into
this repo. Baseline cited in the milestone brief: **43/49** recall, **0/29** FP
(with venv). This change set cannot re-score that private set; reconstructed
fixtures under `fixtures/m3a_*` plus unit tests are the in-repo proxy.

On `fixtures/m3a_corpus_gaps` vs main `873a7be`: main reported 8 findings
(including a bare-`listen()` FP); M3a reports 11 findings (adds
`begin_entered` + `redis_sync_await`, drops bare `listen()`).
`unsafe_async_mutation` stays quiet by design.
