# Public benchmark

Cold/warm timings for `async-doctor` on pinned public asyncio corpora.
Numbers from this VM (Linux x86_64, release binary, isolated
`XDG_CACHE_HOME` per corpus, `VIRTUAL_ENV` pointed at each clone’s `.venv`).

## Results

| Corpus | Commit | Scan | Files | Findings | Candidates | Cold | Warm | Peak RSS | Identical |
| --- | --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | --- |
| litellm | `5029d027a1b6` | `litellm/` | 3009 | 4491 | 739 | ~36s | ~0.5s | ~2.2 GiB | yes |
| httpx | `b5addb64f016` | `httpx/` | 23 | 0 | 0 | 0.49s | 0.01s | 93 MiB | yes |
| starlette | `2745bbc5afa2` | `starlette/` | 36 | 0 | 0 | 0.56s | 0.01s | 95 MiB | yes |

### litellm before → after (parallel pass-2 + whole-scan warm)

| | Cold | Warm | Peak RSS | Findings | Candidates | Cold==Warm |
| --- | ---: | ---: | ---: | ---: | ---: | --- |
| **Before** (`f6f249e`) | 41.1s | 44.9s | 2.11 GiB | 4491 | 739 | yes |
| **After** (default parallel) | ~36s | ~0.5s | ~2.2 GiB | 4491 | 739 | yes |

Cold JSON finding **ids** match the pre-change baseline on litellm (4491).
Warm ≪ cold via the whole-scan report cache (content-hash keyed). Default
pass-2 uses **parallel per-thread salsa DB clones** with per-file
work-stealing; set `ASYNC_DOCTOR_PASS2_BATCH=N` or `locality` to trade wall
time for lower peak RSS.

### Where warm time went (before this change)

`ASYNC_DOCTOR_TIMING=1` on litellm with only the package cache:

| Phase | Cold | Warm (package cache only) |
| --- | ---: | ---: |
| discovery | ~0.03s | ~0.03s |
| extract (ty + follow) | ~41s | ~44s |
| analyze | ~0.8s | ~0.8s |

Warm was not faster: the package cache only skips third-party re-parse;
first-party `SemanticModel` work dominated. After this change, warm is
discovery + fingerprint + scan-cache load (~50 ms) plus JSON emit (~0.4 s).

- **async-doctor:** `0.1.0` (CACHE_SCHEMA_VERSION **v11**)
- **jemalloc:** non-MSVC builds link `tikv-jemallocator`; building the Python
  sdist/wheel that vendors the native binary needs a working `make` (jemalloc's
  autotools build).
- **Host:** Linux 6.12, x86_64 cloud agent VM (multi-core for parallel numbers)
- **Measured:** wall clock via `time.perf_counter`; peak RSS via
  `resource.getrusage(RUSAGE_CHILDREN).ru_maxrss` (Linux KiB)
- **Cold:** empty first-party caches (package cache may be warm). **Warm:** same cache dir reused.
- **Identical:** `cmp -s` of cold vs warm `--format json` output (printed by
  `./bench/run.sh`).
- Pins and install commands: [`bench/repos.toml`](../bench/repos.toml)

Libraries such as httpx / starlette / fastapi are mostly already async, so
zero findings is expected; litellm exercises site-packages follow and
third-party rules under a real uv venv.

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
