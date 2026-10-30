# M2 precision sample notes

Clean-room evaluation of async-doctor M2. Numbers from this agent run
(release binary).

## Fixture rule families (`fixtures/m2_*`)

| Family | Positive | Async / negative |
| --- | --- | --- |
| requests / httpx sync / redis / sqlalchemy Session / psycopg2 / pymysql / pymongo / boto3 / openai / anthropic / kafka / elasticsearch / grpc | reported with async-twin fix hints | `httpx.AsyncClient`, `redis.asyncio`, `AsyncSession`, `AsyncOpenAI` do **not** fire |
| Higher-order `apply(fn)` | chain through `fn()` to sink | — |
| Custom offload wrapper | `real_wrapper` → `asyncio.to_thread` cuts | fake project `to_thread` still reports |
| Nested `inner` collision | only `outer_a.inner` via `handler_a` | `handler_b` / `outer_b.inner` clean |
| Lambda assign / `@property` / genexpr+`next`/`list` | reported | — |
| Imperative routes | `add_api_route` / `add_route` / `Route` / `WebSocketRoute` / router `add_api_route` labeled `route` | — |

## Timing (release, `XDG_CACHE_HOME` fresh then warm)

| Corpus | Files | 1st run | 2nd run |
| --- | ---: | ---: | ---: |
| litellm `litellm/` package | 2997 | 57.8s | 56.3s |
| litellm 1000-file subset | 1000 | 17.5s | 17.0s |
| open-webui `backend/` | 290 | 5.1s | 4.9s |
| `fixtures/m2_third_party_rules` | tiny + stubs | 0.03s | — |

Design targets (~1000 files &lt;10s first / &lt;2s cached) are not yet met on a
cold ty semantic pass; the package-version cache helps when re-following the
same third-party functions, but first-party analysis dominates these corpora
(no project venv with deps). Further perf work belongs with M4 benchmarks.

## FP / FN list (sampled)

### open-webui backend (26 findings)

| Verdict | Examples |
| --- | --- |
| **TP** | `transcription` / `export_models` / `upload_*` doing `open` / `os.makedirs` / `os.remove` on async routes; `process_web_search` / web search helpers calling `socket.getaddrinfo` via `resolve_hostname`; legacy `import_legacy_config_json` `open`+`read` |
| **FP** | None confirmed in the sampled top findings (sync FastAPI `def` routes correctly excluded) |
| **FN** | Dynamic provider/`getattr` paths stay unresolved (hidden unless `--show-unresolved`) — expected |

### litellm `litellm/` (24 findings, 105 candidates)

| Verdict | Examples |
| --- | --- |
| **TP** | Proxy guardrail / MCP / policy endpoints reading YAML/files on async paths (`blocking-file`) |
| **FP** | None confirmed in the first 6 sampled findings |
| **FN** | Large dynamic registry / provider dispatch → unresolved candidates (3195 hidden), not findings — expected precision tradeoff |

### sqlalchemy + redis sync in async code

Covered by `fixtures/m2_third_party_rules`: sync `Session.execute` /
`redis.Redis.get` are findings with async-twin hints; `AsyncSession` /
`redis.asyncio` do not fire.
