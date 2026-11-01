# JSON report contract (`schemaVersion` 2)

Machine-readable output from `async-doctor --format json`. Policy background:
[decisions/2026-10-30-reporting-contract.md](decisions/2026-10-30-reporting-contract.md)
and [decisions/2026-10-31-advice-tier.md](decisions/2026-10-31-advice-tier.md).
CLI flags: [cli.md](cli.md).

| Field | Meaning |
| --- | --- |
| `schemaVersion` | integer; bump on breaking changes (currently **2**) |
| `complete` | false on timeout / parse failure; attach `skipped` |
| `environment` | interpreter/venv/site-packages used; `frameworks` when async frameworks/clients are detected in `site-packages`; `note` when degraded (ratio-aware when a venv is active). `environment.note` and `environment.frameworks` are omitted from JSON when empty (`skip_serializing_if`) |
| `findings[]` / `candidates[]` / `advice[]` | always emitted (even empty); candidates have `reason`; advice is non-blocking (SARIF `note`) |
| `locationLessCandidates[]` | always emitted; missing-third-party aggregate lives here with empty `entry.file`/`chain` and empty `why` |
| `skipped[]` / `sinkSites[]` / `unusedSuppressions[]` / `unresolvedSummary[]` | always emitted (even empty) |
| `hiddenUnresolvedCandidates` / `omittedNonProductionEntries` / `fileCount` / `asyncEntryCount` | always emitted, including `0` |
| `id` | stable sha256 (16-byte hex) of **rule + entry qualname + sink qualname + chain qualnames + occurrence**; **not** file/line/column |
| `rule` | e.g. `blocking-sleep`, `blocking-http` |
| `entry` | async def location + label (`route` / `lifecycle` / `task` / `other`) |
| `chain[]` | shortest evidence chain (`file`, `line`, `text`) |
| `conditions[]` | prove-only tags (see below) |
| `fix` | hint + `fixGroupId` (shared across same root cause) |
| `why` | runnable `async-doctor why file:line --path <cwd-relative>`; empty on location-less entries |

## Condition tags

Emitted only when proven. `unconditional` / `all-paths` never accompany
`if`/`else`/`try`/`match` guards, early-exit guards, or comprehension `if`
filters on the same hop. Literal-constant conditions prune dead branches
(`if False`, `if TYPE_CHECKING`).

| Tag | Meaning |
| --- | --- |
| `unconditional` | No branch/try/match guard on the hop |
| `all-paths` | Reached on every path through the function (when proven) |
| `loop` | Under a loop |
| `async-branch` | Under an `if`/`else`/`try`/`match` (or similar) guard |
| `cache_miss_dependent` | Behind a module/global cache-miss check (`if _cache is None`) |
| `no_running_loop_guard` | Under a proven “no running event loop” guard (nested-event-loop seeds with this tag are dropped) |
| `sync_entry` | FastAPI/Starlette sync `def` route (threadpool) — presentation only |

## Exit codes

Home for exit-code semantics (elsewhere link here):

| Code | Meaning |
| --- | --- |
| `0` | No findings and no unused suppressions |
| `1` | Findings present (even if analysis is incomplete) and/or unused suppressions |
| `2` | Tool error only |

Incomplete analysis sets `complete=false` plus `skipped` / a stderr warning —
**not** exit 2. Candidates and advice never affect the exit code.

**Environment note policy:** no project venv and zero unresolved third-party →
silent. No project venv with unresolved third-party → activate/`VIRTUAL_ENV`
note. Active venv with unresolved/resolved third-party ratio ≥ 25% → “install
project deps/extras” note (does not tell the user to set `VIRTUAL_ENV` again).
