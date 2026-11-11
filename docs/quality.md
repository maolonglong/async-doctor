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
| `async_doctor` CLI / reporters | A- | Dedicated regression coverage across whole-file suppressions, single-file scope, `--changed`/`--diff`, `--fail-on`, `why --format json`, Markdown, and SARIF 2.1.0 schema validation | 2026-11-11 |
| `async_doctor_ty` extract / scan / cache | A- | Pre-indexed site-packages metadata, cross-package follow-target retention, hardened whole-scan fingerprint (`scan-cache-v5`), and 100-trial concurrent cold-cache parity | 2026-11-11 |
| `async_doctor_core` propagate / report | A- | Bound-method param indexing, multi-hop param guards, edge-precise higher-order callback binding, and zero-clone `analyze_owned` propagation | 2026-11-11 |
| `async_doctor_rules` sinks / twins | A- | Aligned `blocking-http` vs `blocking-db` rule docs with tightened `pymysql` / `pymongo` / `grpc` non-IO exclusions | 2026-11-11 |
| Docs / agent map | B+ | Synced CLI, JSON contract, rules, architecture, and benchmark tables; `check_docs.py` enforced | 2026-11-11 |

## Debt tracker

| Debt | Where | Cost of leaving it | Added |
|---|---|---|---|
| Several core modules sit in the 700–779 line band (`pass2.rs`, `resolve.rs`, `suppress.rs`, `families.rs`, `cache.rs`, `propagate/mod.rs`) | `crates/` | Under the 800-line ceiling, but split by responsibility before adding new features to those files | 2026-11-11 |
| GBK / legacy-encoded sources skipped with a warning | `encoding.rs` | Those files invisible to analysis | 2026-11-01 |
| Serial pass-2 panic rollback does not undo changes to existing functions / `follow_targets` (deliberate) | `pass2.rs` | Partial facts after panic; documented behavior | 2026-11-01 |
