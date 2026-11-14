# Quality and tech debt

Grades show where the codebase is solid and where agents should tread carefully.
The debt tracker lists known shortcuts so they get paid down in small steps
instead of piling up.

## CI / test strategy

PR wall-clock target: under ~3 minutes on a warm Actions cache (ideally under
2). Coverage that matters must still run somewhere.

| Job | When | What |
|---|---|---|
| `lint` | every PR / main push | `fmt`, file-length, `check_docs.py`, `clippy` |
| `test` | every PR / main push | `cargo nextest run --workspace` (skips `#[ignore]`), then `cargo test --workspace --doc` |
| `stress` | main push, `workflow_dispatch`, weekly schedule | concurrent cold-cache N=15/20 via `--run-ignored only` |

Local equivalents:

```bash
cargo nextest run --workspace
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

## Grades

| Area | Grade | Why | Updated |
|---|---|---|---|
| `async_doctor` CLI / reporters | A | `--python` end-to-end, stateless ignore-candidate suppression, piped compact JSON, sync-caller fan-in in text/`why`, span-aware `--changed`, `.pre-commit-hooks.yaml`, and `action.yml` | 2026-11-13 |
| `async_doctor_ty` extract / scan / cache | A | Auto-discovered PDM/Poetry/Pipenv/`--python` environments (`env_discovery.rs`), multi-guard + inverted `ParamGuard` & `keyword_literals` (`CACHE_SCHEMA_VERSION` 24), `scan-cache-v8`, and modular sub-580L files | 2026-11-13 |
| `async_doctor_core` propagate / report | A | Conjunction `ParamGuard` pruning, transitive sync-caller fan-in (`syncCallerCount`), `FunctionFact.end_line` + `Class.__init__` span tracking (`item_spans`), and 6 high-precision async anti-pattern advice rules | 2026-11-13 |
| `async_doctor_rules` sinks / twins | A | Expanded third-party sinks/twins (`httpx` module verbs, `boto3`/`botocore` + `Waiter.wait` + `StreamingBody`, `sqlmodel`, `RedisCluster`, `valkey`, `pymongo` collection ops, `AnthropicBedrock`/`AnthropicVertex`, `confluent_kafka`, `groq`, `ollama`, `qdrant_client`), non-IO exclusions, and 3 LLM async correctness advice docs | 2026-11-13 |
| Docs / agent map | A- | Synced CLI, JSON contract, rules, architecture, bundled skill, and completed exec plans; `check_docs.py` enforced with zero warnings | 2026-11-13 |

## Debt tracker

| Debt | Where | Cost of leaving it | Added |
|---|---|---|---|
| GBK / legacy-encoded sources skipped with a warning | `encoding.rs` | Those files invisible to analysis | 2026-11-01 |
| Serial pass-2 panic rollback does not undo changes to existing functions / `follow_targets` (deliberate) | `pass2.rs` | Partial facts after panic; documented behavior | 2026-11-01 |
