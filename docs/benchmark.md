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
| litellm | `5029d027a1b6` | `litellm/` | 3009 | 4915 | 352 | 15.3s | 0.14s | 2.13 GiB | yes |
| httpx | `b5addb64f016` | `httpx/` | 23 | 0 | 0 | 0.22s | 0.01s | 92 MiB | yes |
| starlette | `2745bbc5afa2` | `starlette/` | 36 | 2 | 0 | 0.23s | 0.01s | 94 MiB | yes |

### litellm history (parallel pass-2 + whole-scan warm)

| | Cold | Warm | Peak RSS | Findings | Candidates | Cold==Warm |
| --- | ---: | ---: | ---: | ---: | ---: | --- |
| **Before** (`f6f249e`) | 41.1s | 44.9s | 2.11 GiB | 4491 | 739 | yes |
| **After** (default parallel, this VM) | 15.3s | 0.14s | 2.13 GiB | 4915 | 352 | yes |

Cold JSON finding **ids** match the `c503648` audit baseline on litellm (4915).
Warm ≪ cold via the whole-scan report cache (content-hash keyed). Default
pass-2 uses **parallel per-thread salsa DB clones** that keep one shared
`Storage` for the whole pass (no periodic `fresh_empty` rebuilds — those
regressed RSS; see
[notes/2026-11-salsa-fresh-empty-rss.md](notes/2026-11-salsa-fresh-empty-rss.md)).
Set `ASYNC_DOCTOR_PASS2_BATCH=N` or `locality` to trade wall time for lower
peak RSS.

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

- **async-doctor:** `0.1.0` (CACHE_SCHEMA_VERSION **v15**)
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

### Large monorepo memory (this VM, 2 Nov 2026)

Default path (no `ASYNC_DOCTOR_PASS2_BATCH`) after removing periodic Salsa
rebuilds:

- **`litellm/` package subdirectory** (`5029d027a1b6`, harness walk **3009**
  `.py` files, with venv): **15.3 s** cold / **0.14 s** warm at **2.13 GiB**
  peak RSS, **4915** findings / **352** candidates, cold==warm.
- **`home-assistant/core`** (tip `586a6633`, walk **20,337** `.py` files, with
  venv): completes in **~38 s** cold at **~5.8 GiB** peak RSS (on `56e3fcf` the
  same corpus was OOM-killed above 8 GiB under periodic Salsa rebuilds),
  reporting 5 findings. Prefer a package subdirectory when scanning monorepos
  if memory is tight.

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
