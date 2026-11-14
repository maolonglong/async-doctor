# Make repair evaluation evidence independent of scan inputs

## Purpose

Keep evaluation captures outside the scanned project, preserve original test
contracts, and reject unreviewed or contaminated sessions from isolated
comparison. Validate the new collector on a fixed public project without
claiming that program checks establish agent effectiveness or an optimal repair.

## Progress

- [x] 2026-11-14 UTC: Read the previous evaluation evidence and existing audit
  helpers. Confirmed no permanent collector enforces the identified guardrails.
- [x] 2026-11-14 UTC: Selected the existing Starlette pin
  `2745bbc5afa2f8ce900d290bedca2bc5d3f3cfe8` for real-project validation.
- [x] 2026-11-14 UTC: Implemented external-only capture and a fail-closed gate;
  11 new public-script regressions pass, 53 Python script tests overall.
- [x] 2026-11-14 UTC: Ran 165 unchanged upstream tests and a separate selected
  lifecycle probe: 12 tests pass on asyncio/Trio, with ten completed-Future
  calls, no target waits, and a pending-Future control that does wait.
- [x] 2026-11-14 UTC: Reproduced explicit interpreter symlinks losing their
  venv; added a failing public scan regression, then fixed shared normalization
  for directory, single-file, empty, warm-cache, fingerprint, and twin paths.
- [x] 2026-11-14 UTC: Replayed two valid external captures with identical scope
  and scan/test environment identity. Missing review correctly leaves program
  checks passing but comparison ineligible. The wrong-environment pilot fails.
- [x] 2026-11-14 UTC: Strict release comparison and direct cold/warm replay
  passed all 24 existing cases: 123 diagnostic items and full report bytes
  unchanged, with observed cold misses and warm hits.
- [x] 2026-11-14 UTC: Updated architecture, CLI, quality, and evaluation notes.
  Rechecked 441 Rust tests, 53 Python tests, Clippy, formatting, file budgets,
  and docs. Exported 799 evidence/source files plus a manifest, independently
  verified every hash, then removed task-owned scratch data and helpers.

## Surprises & Discoveries

The initial capture passed tests inside the external venv but reported a system
Python scan environment. Extraction, cache refresh, and environment lookup
canonicalized the interpreter symlink, losing `pyvenv.cfg` and the selected
site-packages. The new gate rejects that pilot capture; it is not valid
same-environment evidence. The regression independently distinguishes a venv
link from an ordinary interpreter alias, which must still follow its target.

Starlette's two reported Future-result sites executed ten times across five
failure cases and both backends; every Future was already done and none called
its `Condition.wait`. A pending-Future control did call `Condition.wait`.
The source ordering and runtime observations do not validate these sites as
actual waits. Implicit completion remains a precision follow-up, not an excuse
to rewrite upstream code or demote all Future-result sinks.

## Decision log

- 2026-11-14 UTC: Add a small standard-library repository script, reusing
  `audit_compare.load_report(strict=True)` rather than inventing a new report
  validator. Initially keep product changes out of scope; never add project
  configuration.
- 2026-11-14 UTC: Capture Python sources plus explicitly selected contract
  files, not every workspace file. Keep metadata, caches, logs, and snapshots
  outside project roots, resolving symlinks before overlap checks.
- 2026-11-14 UTC: Separate successful program checks from comparison
  eligibility. A parent-owned transcript review must bind exact capture hashes;
  a missing review or contamination rejects eligibility. This is an audited
  process, not a security sandbox that prevents thread-reading tools.
- 2026-11-14 UTC: This continuation runs program validation on the existing
  public pin, not another batch of agent trials. Keep the first six outcomes
  unchanged and do not retroactively replace excluded observations.
- 2026-11-14 UTC: The real-project pilot exposed a product environment bug.
  Include the bounded interpreter-path correction rather than working around
  it in the collector. Require scan prefix to match the selected Python's
  runtime prefix in both capture and check, including legacy captures.
- 2026-11-14 UTC: Replay the public corpora in their already-prepared, pinned
  environments. Preserve both the auxiliary natural-environment fixture audit
  and the historical shared-environment selection; do not confuse differing
  dependency inputs with analyzer changes or a new strict preparation check.

## Context

The [first evaluation note](../../notes/2026-11-14-agent-repair-evaluation.md)
records source snapshots entering scan roots, one parent-reading session, and
unrelated project inventory entering exports. The product policy remains the
[evidence-bounded repair decision](../../decisions/2026-11-14-evidence-bounded-repair-guidance.md).

The CLI-and-skill candidate is local commit `73195d4`, not a claimed remote
revision. Existing uncommitted evaluation documents remain part of this
workspace. The collector belongs in `scripts/agent_eval.py` and its public
script tests under `scripts/tests/`. Interpreter normalization belongs in
`env_discovery.rs`, shared by extraction and warm refresh rather than applied
as a collector-only override. No agent or upstream project is allowed to
rewrite the parent-owned original test contract to earn a passing result.

## Plan of work

1. Add failing public-script regressions for disjoint paths, unchanged scan
   scope, protected test changes, missing/contaminated reviews, invalid reports,
   recorded failures/timeouts, and source mutation during collection.
2. Implement capture and check commands. Store exact argv, raw output, exits,
   source/tool/skill hashes, and environment metadata without overwriting prior
   captures. Require explicit source-linked transcript review for eligibility.
3. Reuse the pinned Starlette source, prepare a separate Python environment,
   and capture upstream tests and full-package scans outside the project.
   Inspect lifecycle success and failure behavior before discussing any repair.
4. Record what the checks do and do not prove, preserve reviewable evidence,
   remove temporary helpers, and run Python regressions, workspace tests, and
   documentation checks. No commit, push, or upstream contribution is implied.

## Validation

- `python3 -m unittest discover -s scripts/tests -v`: all existing checks and
  new collector/gate contracts pass through their public command interfaces.
- Nested, ancestor, or symlink-aliased evidence paths fail before collection.
  Before/after scans retain the same source scope when application inputs do
  not change; unrelated `PROJECTS.md` is not copied implicitly.
- Changed original tests or missing/contaminated transcript review cannot
  yield an eligible result, even if the returned test command exits zero.
- Invalid/incomplete scan output, scan errors, and timeouts retain raw logs and
  fail collection. Initial ordinary test failures remain valid before evidence.
- Selected Python runtime prefix must match the report prefix. Venv interpreter
  links preserve their environment on all scan paths; ordinary interpreter
  aliases still resolve correctly, and existing prefix selections stay stable.
- `cargo test --workspace`, `python3 scripts/check_docs.py`, and
  `git diff --check` pass. Upstream validation reports actual selected tests,
  package versions, scan completeness, and remaining uncertainty separately.

## Outcomes & Retrospective

The external collector and comparison gate are ready. Eleven new public-script
regressions join 42 existing checks; the workspace passes 441 Rust tests with
four existing stress tests ignored. Clippy, formatting, file budgets, docs, and
diff whitespace checks pass. The bounded interpreter fix preserves venv links
without changing ordinary aliases, cache schemas, or public JSON shape.

Two real-project captures each pass 165 original tests with matching scope and
Python environment. Missing transcript review correctly rejects comparison
eligibility. Strict release and cold/warm replays preserve all 24 complete
reports. These are program checks, not new Agent-effectiveness evidence; the
first six sessions retain their original outcomes and exclusions.

The review export contains 799 selected source/evidence files plus a manifest;
every archived byte and hash was verified before task-owned venvs, executables,
caches, probes, logs, and research clones were removed. Existing corpus clones,
prepared environments, build output, and earlier evidence exports remain.

Starlette's implicit Future completion remains a separate precision follow-up
in [quality](../../quality.md). One read-only research subagent inspected the
pinned scheduling contracts; the parent ran upstream tests and instrumentation.
No additional repair-evaluation Agent sessions, commits, pushes, releases, or
upstream edits were made. This continuation's code and evaluation documents
remain uncommitted.
