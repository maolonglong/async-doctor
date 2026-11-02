# M4a — Public benchmarks + release pipeline groundwork

## Purpose

Publish cold/warm numbers on pinned asyncio corpora, wire a maturin/PyPI
release workflow, and document install paths so a 0.1 release is mechanically
possible.

## Progress

- [x] Public benchmark harness `bench/run.sh` + `bench/repos.toml`.
- [x] Results table and method notes in [../../benchmark.md](../../benchmark.md).
- [x] Parallel pass-2 + jemalloc + content-hash whole-scan warm (https://github.com/maolonglong/async-doctor/pull/27).
- [x] PyPI release workflow groundwork / README install (https://github.com/maolonglong/async-doctor/pull/25).
- [x] Pre-release robustness and precision PRs #32–#34 merged.

## Surprises & Discoveries

- Package cache alone did not make warm fast; whole-scan cache dropped litellm warm to ~0.5s.
- Full litellm clone (~8686 files) peaked ~6.9GB RSS; prefer package subdirs when memory is tight.

## Decision log

- Decision: keep whole-scan cache; reject per-file incremental / Date: 2026-11-01 / Link: [../../decisions/2026-11-01-reject-incremental-file-cache.md](../../decisions/2026-11-01-reject-incremental-file-cache.md).

## Validation

- `./bench/run.sh` (or `BENCH_ONLY=…`) produces cold/warm JSON with `cmp -s` identical on pinned corpora.
- `cargo test --workspace` green after pre-release PRs.

## Outcomes & Retrospective

Benchmark + packaging groundwork landed. First PyPI publish is tracked by the
active [2026-11-01-first-release.md](2026-11-01-first-release.md)
plan (recall P1s, trusted publisher, fast-rust license, owner confirmation).
