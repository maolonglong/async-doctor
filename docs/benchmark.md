# Public benchmark

Cold/warm timings for `async-doctor` on pinned public asyncio corpora.
Numbers from this VM (Linux x86_64, release binary, isolated
`XDG_CACHE_HOME` per corpus, `VIRTUAL_ENV` pointed at each clone’s `.venv`).

## Results

| Corpus | Commit | Scan | Files | Findings | Candidates | Cold | Warm | Peak RSS | Identical |
| --- | --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | --- |
| litellm | `5029d027a1b6` | `litellm/` | 3009 | 4491 | 739 | 43.6s | 0.47s | 2.22 GiB | yes |
| httpx | `b5addb64f016` | `httpx/` | 23 | 0 | 0 | 0.49s | 0.01s | 93 MiB | yes |
| starlette | `2745bbc5afa2` | `starlette/` | 36 | 0 | 0 | 0.56s | 0.01s | 95 MiB | yes |

### litellm before → after (warm-cache + parallel pass-2)

| | Cold | Warm | Peak RSS | Findings | Candidates | Cold==Warm |
| --- | ---: | ---: | ---: | ---: | ---: | --- |
| **Before** (`f6f249e`) | 41.1s | 44.9s | 2.11 GiB | 4491 | 739 | yes |
| **After** | 43.6s | 0.47s | 2.22 GiB | 4491 | 739 | yes |

Cold JSON finding **ids** match the pre-change baseline on litellm (4491).
Warm ≪ cold via the whole-scan report cache. Incremental one-file edit (file
cache hits + one miss) is ~3–4s and **byte-identical** to a fresh cold on the
edited tree. Default pass-2 keeps one salsa DB (cold ≈ baseline); set
`ASYNC_DOCTOR_PASS2_BATCH=N` or `locality` to trade wall time for lower peak RSS.

### Incremental / all-hit (litellm, package cache warm)

| Mode | Wall | Notes |
| --- | ---: | --- |
| Whole-scan warm | 0.47s | scan-cache hit |
| File-cache all-hit | 2.6s | wipe `scan/` only; merge + follow |
| One-file edit | 3.4s | byte-identical to cold on edited tree |

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

- **async-doctor:** `0.1.0` (CACHE_SCHEMA_VERSION **v12**)
- **Host:** Linux 6.12, x86_64 cloud agent VM (4 cores for these numbers)
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
