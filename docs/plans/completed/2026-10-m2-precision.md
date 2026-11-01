# M2 precision sample notes

## Purpose

Record clean-room M2 precision/recall and cold/warm timings on fixtures and
public corpora so later precision work has a baseline.

## Progress

- [x] 2026-10: Fixture family matrix for `fixtures/m2_*` (third-party rules, redis Pipeline, follow, higher-order, routes).
- [x] 2026-10: Determinism check (5 cold runs, identical SHA-256 JSON).
- [x] 2026-10: Timing table for litellm / open-webui / cache fixtures.
- [x] 2026-10: Sampled FP/FN notes for open-webui and litellm.

## Surprises & Discoveries

- Indexing edges by caller (`O(E)` build) cut litellm cold from ~57.8s to **26.0s** (release, no project venv).
- Warm with only the package cache stayed close to cold until the whole-scan report cache landed (see [../../benchmark.md](../../benchmark.md)).
- Sync FastAPI `def` routes correctly excluded; dynamic provider/`getattr` paths stay unresolved candidates.

### Fixture rule families (`fixtures/m2_*`)

| Family | Positive | Async / negative |
| --- | --- | --- |
| requests / httpx sync / redis / sqlalchemy Session / psycopg2 / pymysql / pymongo / boto3 / openai / anthropic / kafka / elasticsearch / grpc | reported with async-twin fix hints | `httpx.AsyncClient`, `redis.asyncio`, `AsyncSession`, `AsyncOpenAI` do **not** fire |
| redis sync `Pipeline` (real hierarchy stubs) | `pipe.execute()` finding | mixin-defined `pipe.set`/`get`, `client.pipeline()`, `await` asyncio `pipe.execute()` never fire |
| Third-party async (`libz.fetch`) | finding entry is first-party `handler` | site-/dist-packages async defs are never entries |
| Higher-order `apply(fn)` | chain through `fn()` to sink | — |
| Custom offload wrapper | `real_wrapper` → `asyncio.to_thread` cuts | fake project `to_thread` still reports |
| Nested `inner` collision | only `outer_a.inner` via `handler_a` | `handler_b` / `outer_b.inner` clean |
| Lambda assign / `@property` / genexpr+`next`/`list` | reported | — |
| Imperative routes | `add_api_route` / `add_route` / `Route` / `WebSocketRoute` / router `add_api_route` labeled `route` | — |
| Site-packages follow (`m2_sitepkgs_cache`) | `libx.fetch` → `helper` → `time.sleep` | cold JSON == warm JSON |

### Timing (release, isolated `XDG_CACHE_HOME`)

| Corpus | Files | Cold | Warm | Peak RSS (child max) |
| --- | ---: | ---: | ---: | ---: |
| litellm `litellm/` (no project venv) | 2997 | **26.0s** (was 57.8s) | 25.0s | ~1.84 GiB |
| litellm 1000-file subset | 1000 | **8.4s** (was 17.5s) | 8.3s | ~642 MiB |
| open-webui `backend/` | 290 | **4.9s** (was 5.1s) | 4.8s | ~335 MiB |
| `fixtures/m2_sitepkgs_cache` (+ stub `.venv`) | tiny | 33 ms | **23 ms** | ~31 MiB |
| `fixtures/m2_third_party_rules` | tiny + stubs | 34 ms | 29 ms | ~31 MiB |

### FP / FN list (sampled)

#### open-webui backend (26 findings)

| Verdict | Examples |
| --- | --- |
| **TP** | `transcription` / `export_models` / `upload_*` doing `open` / `os.makedirs` / `os.remove` on async routes; `process_web_search` / web search helpers calling `socket.getaddrinfo` via `resolve_hostname`; legacy `import_legacy_config_json` `open`+`read` |
| **FP** | None confirmed in the sampled top findings (sync FastAPI `def` routes correctly excluded) |
| **FN** | Dynamic provider/`getattr` paths stay unresolved (hidden unless `--show-unresolved`) — expected |

#### litellm `litellm/` (sampled)

| Verdict | Examples |
| --- | --- |
| **TP** | Proxy guardrail / MCP / policy endpoints reading YAML/files on async paths (`blocking-file`) |
| **FP** | None confirmed in the first 6 sampled findings |
| **FN** | Large dynamic registry / provider dispatch → unresolved candidates, not findings — expected precision tradeoff |

## Decision log

- Decision: treat unresolved dynamic dispatch as candidates, not findings / Date: 2026-10 / Rationale: false positives are worse than misses ([../../decisions/2026-10-30-reporting-contract.md](../../decisions/2026-10-30-reporting-contract.md)).

## Validation

- `cargo test -p async_doctor_ty` includes `m2_analysis_is_byte_identical_across_runs` (N=5).
- Fixture families under `fixtures/m2_*` pass in the default workspace test run.

## Outcomes & Retrospective

M2 precision baseline recorded. sqlalchemy + redis sync covered by
`fixtures/m2_third_party_rules`; Pipeline buffering is not IO — only
`execute()` blocks. Follow-on corpus gaps moved to M3a plans.
