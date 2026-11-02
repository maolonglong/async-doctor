# First PyPI release (0.1)

## Purpose

Publish `async-doctor` 0.1 to PyPI so users can `uvx async-doctor .` without
building from source, after pre-release audit fixes and owner confirmation.

## Progress

- [x] 2026-11-01: Pre-release audit fixes merged — https://github.com/maolonglong/async-doctor/pull/32, https://github.com/maolonglong/async-doctor/pull/33, https://github.com/maolonglong/async-doctor/pull/34.
- [x] 2026-11-01: P1 recall fixes (boto3 client/resource calls, `Future.result` / `Executor.map` / `wait`, smtplib/ftplib, zipfile/tarfile, glob/`Path.iterdir`, `input()`, chain-through-sinks, genexpr fan-out) — https://github.com/maolonglong/async-doctor/pull/36.
- [x] 2026-11-01: PyPI trusted publisher + `pypi` environment configured for the release workflow.
- [x] 2026-11-01: Removed unlicensed vendored `fast-rust` skill and updated agent docs workflow to v2.
- [x] 2026-11-01: Investigated and optimized peak memory on large monorepos (pass-2 overlay sharing; later release audit found periodic `fresh_empty` rebuilds regress RSS — fixed by removing default-path rebuilds).
- [x] 2026-11-01: Re-ran corpus audit (`litellm` 4,915/4,915 findings match `c503648` baseline; `home-assistant/core` full scan matches `ha-subset` and adds 2 full-repo findings) and refreshed `docs/benchmark.md`.
- [x] 2026-11-01: Ran `release.yml` `workflow_dispatch` dry-run (`41321070358`, all 8 wheel/sdist/dry-run jobs green) and verified the built wheel with `uvx` (`byte-identical` JSON on `fixtures/miss-probe`).
- [x] 2026-11-02: Release-blocker fixes — default-path Salsa rebuild OOM, `anyio.fail_after`/`move_on_after` false positives, `install-skill --force`, hermetic `VIRTUAL_ENV` tests, frameworks/pluralization/`audit_compare` hardening.
- [ ] Owner confirmation before publish.
- [ ] Tag/publish 0.1 and verify `uvx async-doctor --version`.

## Surprises & Discoveries

- Incremental per-file cache rejected on speed grounds despite correctness
  ([../../decisions/2026-11-01-reject-incremental-file-cache.md](../../decisions/2026-11-01-reject-incremental-file-cache.md)).
- Sharing the pass-1 function slice read-only across pass-2 workers cut
  per-file `Vec<FunctionFact>` clone cost. Periodic `fresh_empty()` rebuilds
  added on top of that later **regressed** peak RSS (independent `Arc<Zalsa>`
  arenas retained until teardown) and OOMed large monorepos; the default path
  must keep one Storage per worker
  ([../../notes/2026-11-salsa-fresh-empty-rss.md](../../notes/2026-11-salsa-fresh-empty-rss.md)).
- `anyio.fail_after` / `move_on_after` are sync context managers in anyio 4.x;
  listing them under `sync-with-async-cm` was a false positive.

## Decision log

- Decision: no scheduled doc gardening; enforce docs via `scripts/check_docs.py` in CI / Date: 2026-11-01 / Rationale: owner bootstrap choice for this repo.
- Decision: keep product skill at `skills/async-doctor/SKILL.md`; repo-dev skills under `.agents/skills/` / Date: 2026-11-01 / Rationale: npx skills convention vs agent-docs layout.

## Context

Milestones M0–M4a are in `docs/plans/completed/`. Release workflow lives at
`.github/workflows/release.yml` (maturin trusted publishing intended). Packaging
metadata is in `pyproject.toml`. Remaining quality debt is listed in
`docs/quality.md`.

## Plan of work

1. Owner sign-off on the release-blocker fixes and refreshed benchmarks.
2. Tag `v0.1.0` via the existing release path (humans only — agents do not publish).

## Validation

- `cargo test --workspace` and `python3 scripts/check_docs.py` green.
- After publish: `uvx async-doctor --version` prints 0.1.x; `uvx async-doctor` on a tiny fixture matches a local release binary JSON for findings ids.

## Outcomes & Retrospective

(Active — fill on completion.)
