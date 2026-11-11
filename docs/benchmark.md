# Public benchmark

Cold/warm timings for `async-doctor` on pinned public asyncio corpora.
Numbers from this VM (Linux 6.12 x86_64, **4** cores, **15 GiB** RAM, release
binary, isolated `XDG_CACHE_HOME` per corpus, `VIRTUAL_ENV` pointed at each
clone’s `.venv`). Peak RSS for large corpora is sampled from `/proc/<pid>/status`
`VmHWM` during the scan; small corpora use the bench harness
`RUSAGE_CHILDREN` delta.

## Targets (calibrate against)

| Scenario | Target |
| --- | --- |
| ~1000 project files, first run with site-packages | under 10s |
| Second run (deps / whole-scan cached) | under 2s |
| `--changed` small diff | ~1s |

Public corpora used for calibration: full-stack-fastapi-template, open-webui,
litellm, langgraph, home-assistant core, plus small fixtures per rule/pattern.
Measured numbers below are the current baseline; regress carefully against both
targets and these tables.

## Results

| Corpus | Commit | Scan | Files | Findings | Candidates | Cold | Warm | Peak RSS | Identical |
| --- | --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | --- |
| litellm | `5029d027a1b6` | `litellm/` | 3009 | 5227 | 7421 | 8.40s | 0.38s | 1.11 GiB | yes |
| open-webui | `f7ac7ee10962` | `backend/` | 290 | 33 | 31 | 4.55s | 0.09s | 1.00 GiB | yes |
| httpx | `b5addb64f016` | `httpx/` | 23 | 0 | 0 | 0.11s | 0.01s | 101 MiB | yes |
| starlette | `2745bbc5afa2` | `starlette/` | 36 | 2 | 0 | 0.11s | 0.01s | 108 MiB | yes |
| fastapi | `79cab5bed28b` | `fastapi/` | 52 | 0 | 3 | 0.54s | 0.01s | 182 MiB | yes |

### litellm history (parallel pass-2 + whole-scan warm)

| | Cold | Warm | Peak RSS | Findings | Candidates | Cold==Warm |
| --- | ---: | ---: | ---: | ---: | ---: | --- |
| **Before** (`f6f249e`) | 41.1s | 44.9s | 2.11 GiB | 4491 | 739 | yes |
| **Parallel pass-2** (`c503648`) | 15.3s | 0.14s | 2.13 GiB | 4915 | 352 | yes |
| **Recall + AST clear + waves** (current) | 8.40s | 0.38s | 1.11 GiB | 5227 | 7421 | yes |

Warm ≪ cold via the whole-scan report cache (content-hash keyed; skipped on
single-file targets). Default pass-2 caps workers at 8, clears per-worker
`ParsedModule` ASTs and evicts file contents after per-file visits inside each
worker's local Salsa fork, purges jemalloc dirty arenas between waves, and runs
in coordinated 2,500-file Salsa waves that drop the completed wave's shared
`Arc<Zalsa>` before starting the next (see
[notes/2026-11-salsa-fresh-empty-rss.md](notes/2026-11-salsa-fresh-empty-rss.md)).
Set `ASYNC_DOCTOR_PASS2_BATCH=N` or `locality` to override wave sizing with
serial batched resets.

### Where warm time went (package-cache-only warm)

`ASYNC_DOCTOR_TIMING=1` on litellm with only the package cache (before the
whole-scan report cache):

| Phase | Cold | Warm (package cache only) |
| --- | ---: | ---: |
| discovery | ~0.03s | ~0.03s |
| extract (ty + follow) | ~41s | ~44s |
| analyze | ~0.8s | ~0.8s |

Warm was not faster: the package cache only skips third-party re-parse;
first-party `SemanticModel` work dominated. After the whole-scan report cache,
warm is discovery + fingerprint + scan-cache load (~50 ms) plus JSON emit.

- **async-doctor:** `0.1.1` (`CACHE_SCHEMA_VERSION` **v21**, stripped release binary ~28.1 MiB on macOS arm64)
- **jemalloc:** non-MSVC builds link `tikv-jemallocator`; building the Python
  sdist/wheel that vendors the native binary needs a working `make` (jemalloc's
  autotools build).
- **Host:** Linux 6.12 x86_64 / macOS arm64 (multi-core for parallel numbers)
- **Measured:** wall clock via `time.perf_counter`; peak RSS via
  `resource.getrusage(RUSAGE_CHILDREN).ru_maxrss` (Linux KiB; macOS bytes scaled to KiB)
  for harness runs, or `/proc/<pid>/status` `VmHWM` for large monorepo samples
- **Cold:** empty first-party caches (package cache may be warm). **Warm:** same cache dir reused.
- **Identical:** `cmp -s` of cold vs warm `--format json` output (printed by
  `./bench/run.sh`).
- Pins and install commands: [`bench/repos.toml`](../bench/repos.toml)

Libraries such as httpx are already async, so zero findings is expected;
starlette surfaces two `concurrent.futures.Future.result()` calls inside
`testclient.py`; litellm exercises site-packages follow and third-party rules
under a real uv venv.

### Large monorepo memory (2 Nov 2026)

Default path (no `ASYNC_DOCTOR_PASS2_BATCH`) with `ParsedModule::clear` and
coordinated 1,000-file Pass-2 waves:

- **`litellm/` package subdirectory** (`5029d027a1b6`, harness walk **3009**
  `.py` files, with venv): **8.40 s** cold / **0.38 s** warm at **1.11 GiB**
  peak RSS (down from **2.30 GiB** before AST clearing and wave releases),
  **5227** findings / **7421** candidates, cold==warm.
- **`open-webui/` backend subdirectory** (`f7ac7ee10962`, harness walk **290**
  `.py` files, with venv): **4.55 s** cold / **0.09 s** warm at **1.00 GiB**
  peak RSS (down from **1.54 GiB**), **33** findings / **31** candidates,
  cold==warm.
- **`home-assistant/core`** (tip `586a6633`, walk **20,337** `.py` files, with
  venv): completes in **~38 s** cold (on `56e3fcf` the same corpus was
  OOM-killed above 8 GiB under uncoordinated per-worker Salsa rebuilds),
  reporting 5 findings. Prefer a package subdirectory when scanning monorepos
  if memory is tight.

### Audit remediation delta (11 Nov 2026, macOS arm64)

Same-host comparison before (`40bd0ba`) vs after audit remediation + v21 cache schema:

| Corpus | Cold (before → after) | Warm | Peak RSS | Findings (before → after) | Candidates | Identical |
| --- | ---: | ---: | ---: | ---: | ---: | --- |
| `litellm/` | 11.27s → **10.12s** | 0.21s | 1.62 GiB | 3874 → **4524** (+650, 0 lost) | 7466 → 7614 | yes |
| `backend/` (`open-webui`) | 0.78s → **0.77s** | 0.01s | 326 MiB | 25 → **25** | 32 → 32 | yes |
| `httpx/` | 0.13s → **0.12s** | 0.01s | 97 → **89 MiB** | 0 → **0** | 0 → 0 | yes |
| `starlette/` | 0.13s → **0.16s** | 0.01s | 105 MiB | 2 → **2** | 0 → 0 | yes |
| `fastapi/` | 0.60s → **0.70s** | 0.01s | 178 MiB | 0 → **0** | 3 → 3 | yes |

All +650 newly recalled `litellm` findings trace to bound higher-order credential callbacks (`self._get_or_set_cached_credentials(..., lambda: ...)`) calling synchronous `boto3`/`botocore` STS, file `read()`, `httpx.Client`, and `Future.result()` sinks across 19 sink sites, with zero lost baseline `(entry, sink)` sites across all five corpora.

## Reproduce

```bash
cargo build -p async_doctor --release
./bench/run.sh
```

Requires `git`, `uv`, and a release `async-doctor` binary
(`ASYNC_DOCTOR_BIN=…` to override). Clones land under `bench/.repos/`
(gitignored) and are never committed.

Optional:

```bash
BENCH_ONLY=httpx,fastapi ./bench/run.sh   # subset
BENCH_FORCE_INSTALL=1 ./bench/run.sh      # re-run uv installs
ASYNC_DOCTOR_TIMING=1 …                  # phase spans on stderr
ASYNC_DOCTOR_PASS2_BATCH=N|locality      # lower peak RSS (slower cold)
```
