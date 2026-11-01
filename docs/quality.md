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
| `async_doctor` CLI / reporters | B | Solid CLI tests; SARIF/text paths less exercised than JSON | 2026-11-01 |
| `async_doctor_ty` extract / scan / cache | B | Broad fixtures; pass-2 and cache modules over the 500-line target | 2026-11-01 |
| `async_doctor_core` propagate / report | B | Prove-only core works; `propagate/mod.rs` oversized; memory on huge repos | 2026-11-01 |
| `async_doctor_rules` sinks / twins | B | Good family coverage across stdlib and third-party sinks | 2026-11-01 |
| Docs / agent map | B | Bootstrapped 2026-11-01; gardening is on-demand via audit, not scheduled | 2026-11-01 |

## Debt tracker

| Debt | Where | Cost of leaving it | Added |
|---|---|---|---|
| `propagate/mod.rs` ~634 lines (over 500 target) | `crates/async_doctor_core/src/propagate/mod.rs` | Harder reviews; split-by-responsibility risk | 2026-11-01 |
| `cache.rs` ~515 lines (over 500 target) | `crates/async_doctor_ty/src/cache.rs` | Same | 2026-11-01 |
| `stdlib/sinks.rs` ~615 lines (over 500 target) | `crates/async_doctor_rules/src/stdlib/sinks.rs` | Harder reviews; split by section (sleep/socket/file/locks/…) when next editing | 2026-11-01 |
| Memory on very large repos (Home Assistant core OOM above ~6GB) | extract / pass-2 | Cannot scan some monorepos whole | 2026-11-01 |
| GBK / legacy-encoded sources skipped with a warning | `encoding.rs` | Those files invisible to analysis | 2026-11-01 |
| Serial pass-2 panic rollback does not undo changes to existing functions / `follow_targets` (deliberate) | `pass2.rs` | Partial facts after panic; documented behavior | 2026-11-01 |
