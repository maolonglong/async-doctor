# Bound repair guidance to proven local corrections

## Purpose

Report a Python async problem without prescribing a migration whose suitability
the scan cannot establish. Blocking diagnostics and evidence remain available;
generic thread offload, async conversion, client replacement, and transaction
rewrite instructions do not. An absent recommendation must render cleanly in
JSON, text, Markdown, SARIF, `explain`, and `why`.

## Progress

- [x] 2026-11-14 UTC: Recorded the user's conservative repair policy and traced
  the report, rule-help, reporter, async-twin, and product-skill output paths.
- [x] 2026-11-14 UTC: Preserved the release baseline, 21 complete fixture
  reports, and HTTPX/Starlette/FastAPI strict benchmark reports; inspected
  react-doctor's public documentation without reusing implementation.
- [x] 2026-11-14 UTC: Ran baseline public-interface probes: four expected
  withheld-repair contract failures and a passing protocol positive control.
- [x] 2026-11-14 UTC: Removed speculative repair generation and rendered
  missing hints without empty labels or implied fixes; core tests pass.
- [x] 2026-11-14 UTC: Synchronized rule help, topic docs, decisions,
  architecture, and the product skill without claiming unmeasured optimality.
- [x] 2026-11-14 UTC: Restricted protocol corrections to already-async source
  functions and contextual `why` help; public CLI regression failed before
  each restriction and passed afterwards.
- [x] 2026-11-14 UTC: Passed workspace, nextest, Python, lint, length, and docs
  checks; reviewed all changes across 24 complete baseline/candidate reports
  and verified cold/warm byte parity.

## Surprises & Discoveries

- The current report always contains `fix.hint`, and every reporter assumes it
  contains a recommendation. SARIF even uses it as the diagnostic message.
- `explain` and `why` use `RuleDoc` strings independent of `docs/rules.md`.
  Both must change; editing only the Markdown would leave old instructions.
- Async-twin advice already exists with dependency resolution and sync-caller
  exclusions. Its availability evidence does not prove that a migration is
  semantically equivalent or optimal.
- `async-doctor explain blocking-file` currently lists `to_thread(open, ...)`,
  which does not offload subsequent read, iteration, or close operations.
- React Doctor's public fix guide states that every diagnostic has a
  recommended fix. Optional per-finding help can inherit rule-level guidance;
  it is not evidence that final diagnostics omit advice. Our policy is more
  conservative; actual upstream runtime coverage was not verified.
- A protocol error can be in a sync helper reached from an async entry.
  Replacing its statement with async syntax is illegal without changing the
  helper's API. The source function, not the async entry, controls hint
  applicability; contextual rule help must not reintroduce a withheld hint.

## Decision log

- 2026-11-14 UTC: Preserve schema 2 and the `fix` object, its string `hint`,
  and `fixGroupId`; an empty hint means no recommended edit. This avoids
  breaking consumers merely to represent an absent recommendation.
- 2026-11-14 UTC: Keep diagnostic evidence and async-API availability separate
  from migration recommendations. Do not add a confidence score or claim that
  static analysis proves a globally optimal repair.
- 2026-11-14 UTC: Evaluate local async-only protocol corrections separately
  from changes to scheduling, threads, clients, transactions, or API contracts.
  The latter lack the evidence needed for a prescribed repair.
- 2026-11-14 UTC: Reference react-doctor's public documentation and examples
  only, under the existing clean-room decision; do not reuse implementation.
- 2026-11-14 UTC: Accepted
  [evidence-bounded repair guidance](../../decisions/2026-11-14-evidence-bounded-repair-guidance.md),
  carrying forward prior tier, schema, category, identity, and exit-code policy.
- 2026-11-14 UTC: Keep protocol diagnoses in sync helpers but withhold edits
  requiring API conversion. `why` includes rule repair text only when a selected
  diagnostic establishes that correction; generic `explain` states its limits.

## Context

Baseline is local commit `53b36eb` on `main`, the completed execution-semantics
round. The user authorized this second round but not another commit or push.
The previous commit request covered the first round only.

`async_doctor_core::Fix` owns report hints. At baseline, sink tables in
`crates/async_doctor_rules/src/` supplied blocking repair strings; propagation
added local/installed async-twin and eager-offload instructions. Anti-pattern
advice supplied further recommendations. Text, Markdown, SARIF, and rule help
consumed these strings. The product instructions are
`skills/async-doctor/SKILL.md`. Existing report and advice decisions are
`docs/decisions/2026-10-30-reporting-contract.md` and
`docs/decisions/2026-10-31-advice-tier.md`.

## Plan of work

1. Save a release baseline and selected complete fixture/public-project
   reports. Investigate upstream public interface evidence independently.
2. Extend existing CLI/core regression files with asymmetric cases: blocked
   IO without a fix, async availability without a migration directive, and
   protocol misuse with a narrowly justified local correction.
3. Remove unsupported repair strings at their owners. Keep grouping ids,
   diagnostic ids, conditions, and execution analysis intact. Use actual
   diagnostic evidence rather than an absent fix as reporter message text.
4. Update the skill and docs, including a decision for the changed repair
   policy. Replace human claims that a choke-point edit necessarily clears
   findings with observed reachability counts.
5. Compare complete release reports, review every payload change, and verify
   cold/warm identity. Run workspace, Python, lint, length, and docs checks.

## Validation

- `cargo test --workspace` and `cargo nextest run --workspace -j 4` pass.
- `python3 -m unittest discover -s scripts/tests -v` passes.
- `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
  `./scripts/check_rust_file_length.sh`, and `python3 scripts/check_docs.py` pass.
- CLI regressions fail before the change and pass afterwards. JSON keeps
  schema 2 and stable grouping/evidence while unsupported hints are empty;
  text/Markdown/SARIF and help omit unsupported repair instructions.
- Isolated baseline/candidate corpus audits have no unexplained lost or new
  diagnoses. Expected hint and factual availability changes are reviewed, not
  hidden by changing the strict comparison tool.
- Cold and warm release reports are byte-identical on audited corpora.

## Outcomes & Retrospective

Implemented locally, without a second-round commit, push, release, or remote
workflow. Unsupported repair generation and name-only local twin inference are
removed; diagnosis, resolved availability, grouping, tiers, and exit codes are
preserved. Only same-object protocol corrections inside already-async functions
retain hints. The product skill requires context-supported repair decisions.

`cargo test --workspace` and nextest each pass 440 tests (4 existing stress
tests skipped); Python script tests pass 42. Format, Clippy with warnings denied,
Rust file lengths, Shell syntax, diff whitespace, and docs checks pass. Six
Python 3.11.6 runtime controls confirm local protocol behavior and show that
both async rewrites are illegal inside a sync `def`; this is not optimality proof.

Across 21 fixture families and 3 pinned public corpora, 89 findings, 31 advice,
and 3 location-less candidates keep their ids, grouping, conditions, categories,
locations, score, process exit status, and remaining report metadata. All 123
old hints in those reports are withdrawn; 31 twin availability statements move
to chain evidence. There are no added, removed, or renumbered diagnoses.
Strict audit comparisons deliberately exit 1 for these reviewed payload changes;
the baseline is not rewritten to manufacture a passing gate. All 24 candidate
reports match their warm runs byte-for-byte.

HTTPX/Starlette/FastAPI findings remain 0/2/0. Their existing unresolved optional
modules remain 10/8/7, with one location-less missing-dependency candidate each;
strict preparation is not a complete dependency-health proof. Large corpora,
a Python version matrix, performance claims, and agent-behavior evaluations are
outside this round. Evaluations need separate approval.
