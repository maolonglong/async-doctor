# Public benchmark

Cold/warm timings for `async-doctor` on pinned public asyncio corpora.
Numbers from this VM (Linux x86_64, release binary, isolated
`XDG_CACHE_HOME` per corpus, `VIRTUAL_ENV` pointed at each clone’s `.venv`).

## Results

| Corpus | Commit | Scan | Files | Findings | Candidates | Cold | Warm | Peak RSS |
| --- | --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| litellm | `5029d027a1b6` | `litellm/` | 3009 | 4491 | 739 | 46.4s | 41.2s | 2.11 GiB |
| open-webui | `f7ac7ee10962` | `backend/` | 290 | 33 | 34 | 13.3s | 11.8s | 1.33 GiB |
| httpx | `b5addb64f016` | `httpx/` | 23 | 0 | 0 | 0.43s | 0.38s | 76 MiB |
| starlette | `2745bbc5afa2` | `starlette/` | 36 | 0 | 0 | 0.47s | 0.44s | 76 MiB |
| fastapi | `79cab5bed28b` | `fastapi/` | 52 | 0 | 0 | 1.06s | 0.83s | 127 MiB |

- **async-doctor:** `0.1.0` (workspace at `ff9bdba`)
- **Host:** Linux 6.12, x86_64 cloud agent VM
- **Measured:** wall clock via `time.perf_counter`; peak RSS via
  `resource.getrusage(RUSAGE_CHILDREN).ru_maxrss` (Linux KiB)
- **Cold:** empty cache dir. **Warm:** same cache dir reused.
- Cold and warm JSON were byte-identical for findings/candidates on every
  corpus (`complete=true`).
- Pins and install commands: [`bench/repos.toml`](../bench/repos.toml)

Libraries such as httpx / starlette / fastapi are mostly already async, so
zero findings is expected; litellm and open-webui exercise site-packages
follow and third-party rules under real uv venvs.

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
```
