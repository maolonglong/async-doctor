# Agent repair evaluation is a smoke check, not an effectiveness claim

Six fresh-agent sessions returned contract-compatible code, but this run does
not establish that the new guidance reduces misrepairs. Keep evaluation inputs,
agent isolation, and returned-code verification separate from that claim.

## Observed outcomes

On 2026-11-14, one old/new CLI-and-skill pair handled each of three synthetic
projects using `openai/gpt-6.1-sol`, default effort, and Python 3.11.6. The old
package came from local commit `53b36eb`; the candidate came from `73195d4`.
Neither package's changes were claimed to be on `origin/main`.

- Both protocol sessions changed only synchronous `with`/`for` to the same
  object's async protocols inside an existing async function.
- Both shared-transaction sessions kept the synchronous Session, caller's
  pending changes and rollback, hooks, and thread ownership. They retained two
  database findings rather than substituting an incompatible repair.
- Both whole-unit file-offload sessions left the already-safe code unchanged.
- Parent-owned copies of the original tests passed in all six returned trees:
  three tests per tree, 18 executions total. No original tests were modified.

## Evaluation limitations discovered during the run

The candidate protocol session read the parent transcript to confirm handoff
details. Its code passed independently, but that session is excluded from
strict isolated comparison. It was not replaced with a favorable rerun.
The other five sessions have no recorded parent-transcript or sibling-result
read. This does not turn five observations into a misrepair-rate estimate.

The collector saved source snapshots inside the scanned project. Later scans
included those copies: file counts doubled, and contextual caller counts could
include historical copies. Parent replay in snapshot-free roots confirmed the
application results, but cannot undo what an agent saw while deciding. These
captures are not cold/warm, performance, or identical-scope evidence.

Archived sources also reused incompatible workspace artifacts when first built
against a shared Cargo target directory. Cleaning the four workspace packages
before each successful release build kept compiled third-party dependencies
while forcing the intended source versions to compile.

The orb supplied an unrelated project inventory in `PROJECTS.md`; the collector
copied it with source files. Review exports omit that file and the original
archives containing it. Preserve original archive hashes and verify selected
evidence bytes instead of distributing unrelated project information.

## Evidence and guardrail

The [review report](https://ampcode.com/artifacts/async-doctor-f397d744-44b4-47fb-94df-23407c52fb2e)
holds per-session outcomes, transcript links, input and result hashes, original
test output, and clean-root parent scans. CLI and skill changed together, so
their individual effects were not isolated; there was one sample per cell.

**Guardrail:** this run used frozen input hashes, immutable parent tests, and
explicit exclusion of the parent-reading session. Before another approved
evaluation, use the external collector below and review transcript isolation.
These later guardrails do not retroactively make the first six sessions valid
same-scope or isolated-comparison evidence.

## External capture separates program checks from comparison eligibility

`scripts/agent_eval.py` provides two standard-library commands. It does not
launch agents, automatically review their transcripts, or establish whether
their edits are optimal. `check` gates one session's before/after evidence;
old/new package treatments each need their own matched session, not a direct
comparison of captures from different binaries.

For an approved future session, freeze the original test/contract paths and
capture from the repository root:

```bash
python3 scripts/agent_eval.py capture \
  --project /tmp/case --output /tmp/case-evidence/before \
  --binary "$PWD/target/release/async-doctor" \
  --python /tmp/evaluation-venv/bin/python \
  --skill skills/async-doctor/SKILL.md \
  --protect tests --protect README.md --file pyproject.toml \
  --tests '-m unittest discover -s tests -v'
```

Repeat the same capture with output `/tmp/case-evidence/after` after handling
the case. Keep test arguments, binary, skill, interpreter, dependencies, and
protected paths unchanged. The protected tree is frozen, including additions;
put supplemental validation outside that original grading contract.

- Resolve project/output paths before writing; reject identical, descendant,
  ancestor, or symlink-aliased overlap. Do not overwrite previous captures.
- Snapshot Python sources and explicitly selected non-Python inputs/contracts,
  excluding VCS, venv, and common build/cache trees. An unrelated `PROJECTS.md`
  is not copied unless explicitly selected. This is not a full workspace backup.
- Keep source snapshots, scanner caches, raw stdout/stderr, argv, exits,
  timeouts, CLI/skill hashes, Python prefix/version, and installed-distribution
  names/versions outside the project. Reject source/tool changes during capture.
- Give each actual capture a unique `captureId`. Reject reused capture identity
  or identical metadata, even from different directories or reformatted JSON;
  independently executed unchanged captures remain valid observations.
- Reuse strict report validation and require the scan prefix to match the
  selected interpreter's runtime prefix. This detects wrong-environment scans,
  not complete dependency health or distribution-content integrity.
- Permit ordinary initial test exit 1 as valid before evidence. Capture exit 0
  means collection succeeded, not that tests passed; failed scans, incomplete
  reports, timeouts, and environment mismatches fail collection with raw logs.

The parent must review the full session transcript and write an external JSON
record with `status`, nonempty `evidence`, and `beforeSha256` / `afterSha256`
matching the capture commands' printed metadata hashes. Only set `status=clean`
after actually checking parent/sibling/context reads; cite the reviewed
transcript and findings. `unreviewed`, `contaminated`, missing, or incorrectly
bound reviews cannot earn eligibility.

```bash
python3 scripts/agent_eval.py check \
  --before /tmp/case-evidence/before --after /tmp/case-evidence/after \
  --review /tmp/case-evidence/review.json
```

`checksPassed` describes the after capture's valid scan and passing test
command. `eligibleForIsolatedComparison` additionally requires unchanged
protected contracts and session inputs, valid distinct captures, matching
environment metadata, and accepted transcript-review evidence. Changed tests
can pass while comparison eligibility remains false. Missing review also
returns nonzero even if program checks pass. The JSON review is parent-owned
process evidence, not a security mechanism preventing thread-reading tools.

Final review reproduced copied captures incorrectly earning eligibility when
only directory paths differed. The extended public regression failed for exact
and reformatted copies before the identity check, then passed with independent
unchanged runs as its positive control. Archived captures without `captureId`
still receive environment validation; identical legacy metadata is rejected as
reuse. Capture IDs prevent accidental reuse, not deliberate metadata forgery
or missing transcript isolation. No Agent result is claimed by these probes.

## Real-project validation exposed an interpreter-selection bug

The first external Starlette capture ran all 165 selected original tests in
its venv, but the scanner reported the system prefix. Resolving the venv's
`bin/python` symlink selected its base interpreter, losing `pyvenv.cfg` and
the intended dependencies. That pilot is retained as excluded evidence, and
the new gate rejects it even though its original metadata recorded no errors.

The product now preserves interpreter links identified by `pyvenv.cfg` beside
`bin` / `Scripts`, while ordinary aliases still resolve their target. The same
normalization serves extraction, single-file and empty scans, warm refresh,
cache fingerprints, and twin checks. One public scan regression failed with
the base prefix before the fix and passes with the selected prefix afterward;
it also preserves ordinary interpreter-alias behavior. No cache schema or
public report shape changed.

## Guardrail replay preserves scope without claiming an Agent result

After the fix, two distinct external captures each passed the same 165 original
Starlette tests. Their 91 selected source/contract files, 38 protected files,
Python environment metadata, and full scan JSON match. Each scan covers 34
package files with two findings and one location-less candidate. The selected
venv prefix agrees with the executed Python; seven optional or version-dependent
modules remain unresolved, so this is not complete dependency-health evidence.

With no transcript review supplied, `check` returns exit 1, `checksPassed=true`,
and `eligibleForIsolatedComparison=false`. These are repeated program checks,
not an additional Agent session or a repair-effectiveness comparison. A separate
warm scan of the explicit venv interpreter reports a cache hit and matches the
captured JSON byte for byte.

Release replay used the preserved pre-fix binary and 21 existing fixtures plus
the three public pins in `bench/repos.toml`. With matched dependency selections,
isolated per-case caches, and 2400-second timeouts, strict comparison returns 0:
89 findings, 31 advice items, and three location-less candidates have no changes.
All 24 full baseline/candidate reports are byte-identical. Separate direct CLI
cold/warm runs observe cache misses followed by hits and byte-identical reports
for all 24 cases. This reuses the prepared environments without reinstalling
dependencies; it is not a new preparation check or performance comparison.

## Starlette Future results need a separate precision fix

The fixed source pin is Starlette
`2745bbc5afa2f8ce900d290bedca2bc5d3f3cfe8`. The runtime probe used Python 3.11.6,
AnyIO 4.16.0, HTTPX2 2.16.0, pytest 9.2.0, and Trio 0.35.0. It left upstream
application and test files unchanged, instrumenting only the two reported
`Future.result` calls and their own `Condition.wait` in memory.

Five failure cases ran on both asyncio and Trio: startup failure, middleware
exception before protocol handling, unsupported lifespan state, router startup
failure, and shutdown failure. A healthy lifespan case provided a control.
All 12 upstream tests passed; the ten target calls at `testclient.py:599/614`
observed `done=True`, propagated the expected exceptions, and performed zero
condition waits. A separate pending-Future control did wait. The 12 tests
overlap the 165-test capture suite; do not sum them as unique test coverage.

Source ordering explains this observation: Starlette sends a final `None`
through an unbounded stream; AnyIO's send checkpoints before publication and
does not suspend afterward. The portal finishes its concurrent Future before
the consumer resumes in these paths. References:
[Starlette lifecycle](https://github.com/Kludex/starlette/blob/2745bbc5afa2f8ce900d290bedca2bc5d3f3cfe8/starlette/testclient.py#L557-L624),
[AnyIO Future completion](https://github.com/agronholm/anyio/blob/0ddd02d6a44a68cfdf0210783c2a2a1b02334d9a/src/anyio/from_thread.py#L242-L289),
and [stream publication](https://github.com/agronholm/anyio/blob/0ddd02d6a44a68cfdf0210783c2a2a1b02334d9a/src/anyio/streams/memory.py#L224-L289).

The current scanner still reports the two sites. These observations do not
validate them as blocking waits, justify a blind `await self.task` rewrite,
or prove all Future-result calls safe. Existing explicit `.done()` guards are
handled; implicit completion through coordination needs a separate precision
proof. It is recorded in [quality](../quality.md), not hidden with suppression
or a blanket sink downgrade. No additional repair-evaluation Agent sessions
were run, and the original six outcomes and limitations remain unchanged.
