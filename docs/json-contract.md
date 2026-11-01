# JSON report contract (`schemaVersion` 2)

Machine-readable output from `async-doctor --format json`. Policy background:
[decisions/2026-10-30-reporting-contract.md](decisions/2026-10-30-reporting-contract.md)
and [decisions/2026-10-31-advice-tier.md](decisions/2026-10-31-advice-tier.md).
CLI flags: [cli.md](cli.md).

| Field | Meaning |
| --- | --- |
| `schemaVersion` | integer; bump on breaking changes (currently **2**) |
| `complete` | false on timeout / parse failure; attach `skipped` |
| `environment` | interpreter/venv/site-packages used; `note` when degraded (ratio-aware when a venv is active). `environment.note` is omitted from JSON when empty (`skip_serializing_if`) |
| `findings[]` / `candidates[]` / `advice[]` | always emitted (even empty); candidates have `reason`; advice is non-blocking (SARIF `note`) |
| `locationLessCandidates[]` | always emitted; missing-third-party aggregate lives here with empty `entry.file`/`chain` and empty `why` |
| `skipped[]` / `sinkSites[]` / `unusedSuppressions[]` / `unresolvedSummary[]` | always emitted (even empty) |
| `hiddenUnresolvedCandidates` / `omittedNonProductionEntries` | always emitted, including `0` |
| `id` | stable hash of rule + entry + sink (baseline-friendly); **not** line/column |
| `rule` | e.g. `blocking-sleep`, `blocking-http` |
| `entry` | async def location + label (`route` / `lifecycle` / `task` / `other`) |
| `chain[]` | shortest evidence chain (`file`, `line`, `text`) |
| `conditions[]` | e.g. `unconditional`, `loop` |
| `fix` | hint + `fixGroupId` (shared across same root cause) |
| `why` | runnable `async-doctor why file:line --path <cwd-relative>`; empty on location-less entries |

**Environment note policy:** no project venv and zero unresolved third-party →
silent. No project venv with unresolved third-party → activate/`VIRTUAL_ENV`
note. Active venv with unresolved/resolved third-party ratio ≥ 25% → “install
project deps/extras” note (does not tell the user to set `VIRTUAL_ENV` again).

**Exit codes:** `0` clean, `1` findings (or unused suppressions), `2` tool
error. Incomplete analysis is `complete=false`, not exit 2. Candidates and
advice never affect the exit code.
