# Evaluate agent repairs on evidence-bounded guidance

## Purpose

Compare the previous and current CLI-and-skill packages in six fresh Agent
threads. Check whether agents make justified local repairs, preserve shared
transaction contracts, and leave already-safe whole-unit file offload alone.
This bounded experiment tests observable behavior, not globally optimal fixes.

## Progress

- [x] 2026-11-14 UTC: Committed the verified second round locally as
  `73195d4`; no push, release, or remote workflow.
- [x] 2026-11-14 UTC: Confirmed approval for the previously proposed six-thread
  evaluation on `openai/gpt-6.1-sol`, using the default effort and identical tools.
- [x] 2026-11-14 UTC: Prepared and validated three executable Python fixtures,
  exact package inputs, neutral prompts, and a preregistered scoring rubric.
- [x] 2026-11-14 UTC: Launched six fresh no-project threads with the same model,
  default effort, and no optional serving features; neutral run ids R1 through R6.
- [x] 2026-11-14 UTC: Completed all six sessions. Excluded R2 from strict
  isolated comparison because it read parent context; did not replace it.
- [x] 2026-11-14 UTC: Retrieved and inspected actual edits and logs. Parent
  replay passed three immutable tests per returned tree, 18 executions total;
  clean-root scans were complete with final findings 0/0/2/2/0/0.
- [x] 2026-11-14 UTC: Published outcomes and limitations in the existing review
  artifact. Exported 287 selected evidence files and verified every byte against
  the originals, retaining original result-archive hashes and source inputs.
- [x] 2026-11-14 UTC: Removed task-owned builds, venv, caches, input copies,
  replay trees, and temporary helpers after export verification. Repository
  documentation checks passed with 48 docs, zero errors, and zero warnings.

## Surprises & Discoveries

Preflight confirmed two protocol findings in each package and failing runtime
tests; the same-object correction passes all three tests. Both transaction
packages report two database findings with all three runtime tests passing;
the old package prescribes installed AsyncSession or a new worker-owned unit,
while the candidate supplies empty hints. Whole-unit file offload is clean
and passes all three tests in both packages. Incorrect Session transfer fails
two tests; partial file offload fails the worker-lifetime test.

The first archived-source build reused incompatible current-workspace Cargo
artifacts in the shared target directory. Cleaning only the four workspace
packages before each release build produced complete isolated-source builds;
third-party compiled dependencies were retained. Both successful build logs
and executable hashes are recorded. This is an evaluation build-procedure
issue, not a reported product defect.

R2, the candidate protocol session, read the full parent transcript to confirm
handoff details. Its returned code passes original tests, but its decision is
not eligible for strict isolated comparison. The other five sessions have no
recorded parent-context or sibling-result read; that is not a reliability rate.

The collector put historical source snapshots inside scan roots. Later scans
included those copies, doubling file counts and potentially affecting caller
counts. Independent clean-root replay confirms the returned code, but cannot
remove the possible influence of extra material on the original agent decision.

Source capture also included the orb's unrelated `PROJECTS.md`. The review
export omits that inventory and original archives containing it, preserves
application/test/log bytes, and retains original archive hashes. The exported
files were checked for unrelated project URLs before disposable data was removed.

Both packages repaired the local protocol mismatch, declined an unsupported
transaction rewrite, and preserved already-safe file offload. No observed
misrepair or improvement separates them in this small, explicit-contract trial.
The product policy is recorded in the
[accepted repair decision](../../decisions/2026-11-14-evidence-bounded-repair-guidance.md).

## Decision log

- 2026-11-14 UTC: Compare the complete old/new CLI-and-skill packages, rather
  than attributing a difference to skill wording alone. Both changed together
  in round two; six sessions cannot isolate both factors independently.
- 2026-11-14 UTC: Use one fresh thread per treatment/case, all on the same model
  and default effort. Do not retry disappointing outcomes or add cases after
  observing results. Preserve setup failures as such.
- 2026-11-14 UTC: Give agents identical public contracts and tests within each
  pair. Keep the scoring rubric and other sessions' outcomes out of their
  inputs. Parent-owned immutable tests check returned application code.
- 2026-11-14 UTC: Use exact binaries and skill bytes from this thread's local
  commits, transferred with thread file tools. New orbs cannot read this
  checkout or its unpushed commits by SHA alone.
- 2026-11-14 UTC: Keep R2's verified code outcome but exclude its isolated
  comparison result. Do not rerun a behavioral cell after observing the result.
- 2026-11-14 UTC: Use clean-root parent replay for application verification;
  do not claim it repairs the original scope/isolation problems or establishes
  cold/warm parity, performance, causal improvement, or a misrepair rate.
- 2026-11-14 UTC: Export selected evidence with file hashes, omitting the
  unrelated inventory and raw result archives. Record the original archive
  hashes; retained source inputs can rebuild semantics, not identical model runs.

## Context

The baseline is local `main` commit `53b36eb`; the candidate is local `main`
commit `73195d4`. Neither is claimed to be on `origin/main`. Both preserve
Python execution analysis; the candidate withdraws speculative repair hints
and requires context-supported repair choices in `skills/async-doctor/SKILL.md`.

Working inputs and results lived under `.amp/in/agent-eval/` and were removed
after verified evidence export. The existing
[review artifact](https://ampcode.com/artifacts/async-doctor-f397d744-44b4-47fb-94df-23407c52fb2e)
links the selected evidence archive, concise output, and all six transcripts.
The parent owned scoring and aggregation; each child received one project and
one package. Intended isolation was breached by R2 as recorded above.

## Plan of work

1. Validate an async-only resource/iterator with a locally repairable mismatch,
   a shared caller-owned SQLAlchemy transaction with an available async API,
   and self-contained file reading already offloaded as one synchronous unit.
   Confirm actual initial scanner and Python outcomes before launching agents.
2. Package one fixture, its public contracts/tests, one exact CLI, and its
   matching skill per thread. Record SHA-256 hashes and the same task prompt.
3. Launch six temporary no-project threads and let each independently inspect
   and handle its project's async problems. Use wait-for-threads, not reply-back.
4. Download final application files, patches, scans, environment, and test
   evidence, keeping the sessions available for review. Rerun pristine fixture
   tests against returned application files and inspect any behavioral changes.
5. Publish per-case results and evidence. Preserve reproducible source inputs
   and outcomes in a review archive; remove transient builds and helpers.

## Validation

- Protocol case: initial runtime tests fail and the CLI identifies both local
  protocol mismatches. A valid repair passes the original tests for values,
  exceptions, cancellation, and cleanup without changing its public async API.
- Transaction case: original tests pass but a blocking database diagnosis
  remains. The supplied synchronous Session, caller transaction, hooks, and
  synchronous/async entry contracts must remain intact. Merely installing or
  resolving AsyncSession does not prove a compatible migration.
- File-offload control: original tests and scans are clean. Open, iteration,
  parsing, and close run together on a worker, with values/errors preserved.
- Each returned project is rescanned and tested by the parent with the original
  tests; edited tests or missing evidence do not count as success. Report code
  changes and unsupported repair recommendations separately.
- `python3 scripts/check_docs.py` and `git diff --check` pass for repo documents.

## Outcomes & Retrospective

All six returned trees passed parent-owned original runtime tests without
changing those tests. This is nine distinct original tests executed twice,
not 18 new tests. R1 and R2 changed only `app.py` for same-object async protocol
use; R3/R4 and R5/R6 left application code unchanged. Both transaction scans
retained two genuine database findings and exit 1 without suppression.

The trial is a bounded smoke check, not evidence that the candidate improves
repair effectiveness. One sample per cell, R2's isolation exclusion, and the
collector scope flaw prevent broader claims; no replacement run was launched.
Supplemental child probes remain child evidence: the parent independently
replayed original contract tests and full scans, not every child experiment.

No product change was needed after evaluation. Before any further approved
evaluation, keep evidence outside scan roots and check context isolation. This
debt is recorded in [quality](../../quality.md) and the
[evaluation note](../../notes/2026-11-14-agent-repair-evaluation.md).

The user's commit request covered completed round-two work. These subsequent
evaluation documents remain uncommitted. Nothing was pushed or released, and
no evaluation thread was archived.
