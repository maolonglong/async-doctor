# First PyPI release (0.1)

## Purpose

Publish `async-doctor` 0.1 to PyPI so users can `uvx async-doctor .` without
building from source, after pre-release audit fixes and owner confirmation.

## Progress

- [x] 2026-11-01: Pre-release audit fixes merged — https://redirect.github.com/maolonglong/async-doctor/pull/32, https://redirect.github.com/maolonglong/async-doctor/pull/33, https://redirect.github.com/maolonglong/async-doctor/pull/34.
- [ ] P1 recall fixes (see debt rows in [../../quality.md](../../quality.md): boto3 client calls, `concurrent.futures.Future.result()`, smtplib/ftplib, zipfile/tarfile, glob/`Path.iterdir`, `input()`, chain-through-sinks).
- [ ] PyPI trusted publisher + `pypi` environment configured for the release workflow.
- [ ] Resolve vendored `fast-rust` skill license (no upstream license) before open-sourcing / public release.
- [ ] Owner confirmation before publish.
- [ ] Tag/publish 0.1 and verify `uvx async-doctor --version`.

## Surprises & Discoveries

- Incremental per-file cache rejected on speed grounds despite correctness
  ([../../decisions/2026-11-01-reject-incremental-file-cache.md](../../decisions/2026-11-01-reject-incremental-file-cache.md)).
- Home Assistant core OOM above ~6GB remains an open memory risk for “scan the
  whole monorepo” workflows.

## Decision log

- Decision: no scheduled doc gardening; enforce docs via `scripts/check_docs.py` in CI / Date: 2026-11-01 / Rationale: owner bootstrap choice for this repo.
- Decision: keep product skill at `skills/async-doctor/SKILL.md`; repo-dev skills under `.agents/skills/` / Date: 2026-11-01 / Rationale: npx skills convention vs agent-docs layout.

## Context

Milestones M0–M4a are in `docs/plans/completed/`. Release workflow lives at
`.github/workflows/release.yml` (maturin trusted publishing intended). Packaging
metadata is in `pyproject.toml`. Remaining quality debt is listed in
`docs/quality.md`.

## Plan of work

1. Land P1 recall fixtures/fixes without loosening the FP bar.
2. Confirm GitHub `pypi` environment + trusted publisher against the release workflow.
3. License or remove `.agents/skills/fast-rust` before making the repo public.
4. Owner sign-off; then tag `v0.1.0` via the existing release path (humans only — agents do not publish).

## Validation

- `cargo test --workspace` and `python3 scripts/check_docs.py` green.
- After publish: `uvx async-doctor --version` prints 0.1.x; `uvx async-doctor` on a tiny fixture matches a local release binary JSON for findings ids.

## Outcomes & Retrospective

(Active — fill on completion.)
