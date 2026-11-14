# Quality and tech debt

Grades show where the codebase is solid and where agents should tread carefully.
The debt tracker lists known shortcuts so they get paid down in small steps
instead of piling up.

## CI / test strategy

PR wall-clock target: under ~3 minutes on a warm Actions cache (ideally under
2). Coverage that matters must still run somewhere.

| Job | When | What |
|---|---|---|
| `lint` | every PR / main push | `fmt`, file-length, `check_docs.py`, Python script regressions, `clippy` |
| `test` | every PR / main push | `cargo nextest run --workspace` (skips `#[ignore]`), then `cargo test --workspace --doc` |
| `stress` | main push, `workflow_dispatch`, weekly schedule | concurrent cold-cache N=15/20 via `--run-ignored only` |

Local equivalents:

```bash
cargo nextest run --workspace
python3 -m unittest discover -s scripts/tests -v
cargo nextest run -p async_doctor --test concurrent_cache --profile stress \
  --run-ignored only \
  -E 'test(concurrent_cold_scans) and not test(/_stress$/)'
cargo test -p async_doctor --test concurrent_cache -- --ignored   # N=15/20 + N=100
```

Build profile: workspace crates use `opt-level = 1` in `dev`/`test`; all
non-workspace packages use `opt-level = 3` so ty/ruff analysis in tests is not
pathologically slow. Cache: `Swatinem/rust-cache` with per-job `shared-key`
values (`ci-lint`, `ci-test`) and `save-if` only on `main` (PRs restore, never
save). Keys are split because GitHub Actions cache entries are write-once: a
single shared key lets the faster `lint` job seal the entry before `test` can
save test-profile artifacts. Concurrent cold-cache detail:
[notes/2026-10-concurrent-cache-stress.md](notes/2026-10-concurrent-cache-stress.md).

Extraction, propagation, rule, and cache changes also need a release corpus
comparison with reviewed diagnostic deltas and cold/warm parity. Use the
opt-in strict benchmark/audit modes described in [benchmark.md](benchmark.md);
they reject invalid/incomplete reports but do not prove dependency health.

## Grades

| Area | Grade | Why | Updated |
|---|---|---|---|
| `async_doctor` CLI / reporters | A | `--python` end-to-end, stateless ignore-candidate suppression, piped compact JSON, sync-caller fan-in in text/`why`, span-aware `--changed`, `.pre-commit-hooks.yaml`, and `action.yml` | 2026-11-13 |
| `async_doctor_ty` extract / scan / cache | A | Environment discovery, typed asyncio callbacks, local generator consumption, MRO context methods, coroutine/protocol separation, use-def parameter provenance (`CACHE_SCHEMA_VERSION` 25), and unchanged `scan-cache-v8` | 2026-11-14 |
| `async_doctor_core` propagate / report | A | Offload-wrapper local work retained, coroutine/async-generator distinction, path-bound `ParamGuard` forwarding, transitive sync-caller fan-in, and span-aware filtering | 2026-11-14 |
| `async_doctor_rules` sinks / twins | A | Expanded third-party sinks/twins (`httpx` module verbs, `boto3`/`botocore` + `Waiter.wait` + `StreamingBody`, `sqlmodel`, `RedisCluster`, `valkey`, `pymongo` collection ops, `AnthropicBedrock`/`AnthropicVertex`, `confluent_kafka`, `groq`, `ollama`, `qdrant_client`), non-IO exclusions, and 3 LLM async correctness advice docs | 2026-11-13 |
| Docs / agent map | A- | Synced CLI, JSON contract, rules, architecture, bundled skill, and completed exec plans; `check_docs.py` enforced with zero warnings | 2026-11-13 |

## Debt tracker

| Debt | Where | Cost of leaving it | Added |
|---|---|---|---|
| GBK / legacy-encoded sources skipped with a warning | `encoding.rs` | Those files invisible to analysis | 2026-11-01 |
| Serial pass-2 panic rollback does not undo changes to existing functions / `follow_targets` (deliberate) | `pass2.rs` | Partial facts after panic; documented behavior | 2026-11-01 |
| Deferred-source coverage stays deliberately local | `collector/generators.rs`, `collector/calls/callbacks.rs` | Cross-scope/ambiguous generator sources, named `next`, arbitrary consumers, and local partial callback aliases can hide paths until their bindings are provable | 2026-11-14 |
| Agent repair effectiveness is not established | [Evaluation note](notes/2026-11-14-agent-repair-evaluation.md) | The external collector now enforces capture checks and requires transcript-review evidence, but cannot repair the first trial's scope/isolation defects or establish effectiveness; further Agent trials need separate approval | 2026-11-14 |
| Implicit Future completion is not modeled | [Starlette runtime evidence](notes/2026-11-14-agent-repair-evaluation.md#starlette-future-results-need-a-separate-precision-fix) | Two reported lifecycle sites used already-completed Futures with no waits in exercised failure paths; explicit `.done()` guards work, but coordination-based completion needs a separate proof rather than a global sink downgrade | 2026-11-14 |
