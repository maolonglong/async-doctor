# JSON report contract (`schemaVersion` 2)

Machine-readable output from `async-doctor --format json` (pretty-printed on
TTYs; compact single-line JSON when stdout is piped or redirected). Policy
background:
[decisions/2026-10-30-reporting-contract.md](decisions/2026-10-30-reporting-contract.md)
and [decisions/2026-10-31-advice-tier.md](decisions/2026-10-31-advice-tier.md).
CLI flags: [cli.md](cli.md).

| Field | Meaning |
| --- | --- |
| `schemaVersion` | integer; bump on breaking changes (currently **2**) |
| `complete` | false on timeout / parse failure; attach `skipped` |
| `score` | deterministic `0..=100` health score (`100` when clean; `>= 85` ("Great") iff zero findings and zero unused suppressions; see **Health score** below) |
| `environment` | interpreter/venv/site-packages used; `frameworks` when async frameworks/runtimes (FastAPI, Starlette, Django, aiohttp, anyio, Trio, … — not client libs like httpx/redis) are detected in `site-packages`; `unresolvedThirdPartyModules` (up to 10 sorted top-level missing third-party package names); `note` when degraded (ratio-aware when a venv is active). `environment.note`, `environment.frameworks`, and `environment.unresolvedThirdPartyModules` are omitted from JSON when empty (`skip_serializing_if`); `environment.followBudgetSkipped` counts third-party callees not followed because the follow budget was reached — omitted when 0, and it never flips `complete` |
| `findings[]` / `candidates[]` / `advice[]` | always emitted (even empty); candidates have `reason` (e.g. `conditional`, `metadata`, `short-lock`, `cheap-path-op`, `unresolved`); advice is non-blocking (SARIF `note`) |
| `locationLessCandidates[]` | always emitted; missing-third-party aggregate lives here with empty `entry.file`/`chain` and empty `why` |
| `skipped[]` / `sinkSites[]` / `unusedSuppressions[]` / `unresolvedSummary[]` | always emitted (even empty); `sinkSites[]` attributes merged `open` + `read`/`write` chains to the primary `open` hop, and includes optional `syncCallerCount` (omitted when 0) and `sampleSyncCaller` (`file:line`, omitted when absent); see **Reachability summaries** below |
| `hiddenUnresolvedCandidates` / `omittedNonProductionEntries` / `fileCount` / `asyncEntryCount` / `omittedFindings` / `omittedCandidates` / `omittedAdvice` | always emitted, including `0` — the `omitted*` trio counts entries cut by `--max-findings` (0 without the flag) |
| `id` | stable sha256 (16-byte hex) of **rule + entry qualname + sink qualname + chain qualnames + occurrence**; **not** file/line/column |
| `rule` | e.g. `blocking-sleep`, `blocking-http` |
| `category` | advice only; `performance` or `correctness`, informational and unchanged by the presence or absence of a repair hint |
| `entry` | async def location + label (`route` / `lifecycle` / `task` / `async_def`) |
| `chain[]` | shortest evidence chain (`file`, `line`, `text`) |
| `conditions[]` | prove-only tags (see below) |
| `fix` | always-present object with string `hint` and stable `fixGroupId`; `hint: ""` means no recommended edit; grouping remains available even without a recommendation |
| `why` | runnable `async-doctor why file:line --path <cwd-relative>` (locations with spaces are quoted); empty on location-less entries. Running `async-doctor why <file:line> --format json` emits `{ schemaVersion, location: { file, line }, findings, candidates, advice, rules }` after applying inline suppressions |

## Diagnosis and repair hints

`schemaVersion` remains **2**. Every finding, candidate, and advice item,
including a location-less candidate, keeps `fix`, its string `hint`, and its
stable `fixGroupId`. An empty hint is `""`, not `null` or an omitted field. It
means the scan recommends no edit; the diagnosis, evidence, tier, ids, and
exit-code semantics remain intact. A shared `fixGroupId` groups diagnostics,
not a proven repair that resolves them all.

Non-empty repair hints are limited to `sync-with-async-cm` and
`sync-for-async-iterable`: a local correction selects the supported async-only
protocol on the same object, only inside an already-async function. A sync
helper reached from an async entry still receives the diagnosis, but no hint
requiring an API conversion. This is not a claim of a globally optimal repair.

`why` limits rule-level repair text to corrections present in the selected
diagnostics. Its `rules[].fix` remains a string, empty when no selected
diagnostic establishes a correction for that rule.

`offload-has-async-twin` records same-package async-symbol availability in the
scanned environment. Its `performance` category remains for compatibility,
not as a measured performance result or a recommendation to replace a safe
offload. Its repair hint is empty.

## Reachability summaries

`sinkSites[].findingCount` and `entryCount` count the analyzed finding chains
and async entries reaching a site. Optional `chokePoint*` fields identify a
shared intermediate hop in those chains. These are reachability summaries,
not predictions of how many findings an edit will clear.

`syncCallerCount` counts distinct sync-only project callers reaching the site's
sync helper in the analyzed graph, directly or through sync helpers.
`sampleSyncCaller` is one observed call location, not a complete caller list.
Zero or omitted counts do not prove that no synchronous callers exist:
external, dynamic, or otherwise unresolved callers may be absent from the
graph. Neither these counts nor `complete=true` establish complete caller
coverage or permission to convert an API to async.

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
| `cache_miss_dependent` | Behind a module/global cache-miss check (`if _cache is None`), or inside a `functools.cache` / `lru_cache` helper |
| `no_running_loop_guard` | Under a proven “no running event loop” guard (nested-event-loop seeds with this tag are dropped) |
| `sync_entry` | FastAPI/Starlette sync `def` route (threadpool) — presentation only |

## Exit codes

Home for exit-code semantics (elsewhere link here):

| Code | Meaning |
| --- | --- |
| `0` | No findings and no unused suppressions (or `--fail-on none`) |
| `1` | Findings present (even if analysis is incomplete) and/or unused suppressions |
| `2` | Tool error only |

Incomplete analysis sets `complete=false` plus `skipped` / a stderr warning —
**not** exit 2. Candidates and advice never affect the exit code. `--fail-on none`
forces exit `0` on findings/unused suppressions while keeping exit `2` for tool
errors. `async-doctor why file:line` (both text and `--format json`) applies
inline suppressions first, returns `1` when the location matches any active
finding (candidates/advice alone stay `0`), and `2` on tool errors.

**Environment note policy:** no project venv and zero unresolved third-party →
silent. No project venv with unresolved third-party → activate/`VIRTUAL_ENV`
note. Active venv with unresolved/resolved third-party ratio ≥ 25% → “install
project deps/extras” note (does not tell the user to set `VIRTUAL_ENV` again).

## Health score

`Report::compute_score` (`crates/async_doctor_core/src/report.rs`) computes `score` (`0..=100`):

- **Bands:** `85..=100` (`Great`, iff `exit_code() == 0`), `60..=84` (`Needs work`), `0..=59` (`Critical`).
- **Clean / advice-only:** when `findings` and `unusedSuppressions` are both empty, `score = 100 - min(round(6 * ln(1 + advice_count)), 15)`.
- **With findings or unused suppressions:** starts from `base_error_penalty = 16` (so `score <= 84`), then subtracts:
  - `site_penalty = min(round(9.5 * ln(1 + sum(rule_weight(site)))), 45)` across unique `sinkSites` (`1.5` for `nested-event-loop` / `sync-with-async-cm` / `offload-coroutine`, `1.35` for `blocking-sleep` / `blocking-subprocess`, `1.2` for `blocking-http` / `blocking-db` / `blocking-socket`, `1.1` for `blocking-lock`, `0.85` for `blocking-file`);
  - `spread_penalty = min(round(16 * sqrt(affected_entries / total_entries) + 2 * ln(1 + finding_count)), 25)`;
  - `unused_penalty = min(4 * unused_count, 12)` and `advice_penalty <= 15`.
