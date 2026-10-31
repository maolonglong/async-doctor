# async-doctor

**Your agent blocks the event loop. This catches it.**

Zero-config static analysis for Python: finds sync/blocking IO reachable from
`async def` code, and reports evidence an agent can act on.

## Install

```bash
# Once published to PyPI:
uvx async-doctor .
pipx run async-doctor .
pip install async-doctor && async-doctor .

# From this repo (wheel):
maturin build --release
uvx --from ./target/wheels/async_doctor-*.whl async-doctor --version

# From source (Rust toolchain required):
cargo install --git https://github.com/maolonglong/async-doctor async_doctor
```

## Quick start

```bash
async-doctor .
async-doctor --format json .
async-doctor --format sarif . > async-doctor.sarif

async-doctor explain blocking-sleep
async-doctor why helpers.py:5 --path fixtures/m1_multihop

# CI / agent loops (full analysis; filtered reporting):
async-doctor --changed .
async-doctor --changed=origin/main .
async-doctor --write-baseline baseline.json .
async-doctor --baseline baseline.json .
```

Every finding includes the full call chain (`file:line` per hop), a concrete fix
hint, a stable rule id, a `docsAnchor` into [docs/rules.md](docs/rules.md), and
a runnable `why` command. No config file — flags and inline comments only.

```python
time.sleep(1)  # async-doctor: ignore[blocking-sleep]: brief sync boundary in CLI wrapper
```

Unused ignores fail the run. Exit `0` = clean, `1` = findings (or unused
suppressions), `2` = tool error. Incomplete analysis sets `complete=false` and
does **not** use exit 2. Candidates never affect the exit code.

By default, entries under `tests/` / `docs/` / `examples/` / `scripts/` /
hidden dirs are skipped (`--include-non-production` to include).
`reason=unresolved` candidates are hidden unless `--show-unresolved`.

## What it detects

- Sync/blocking IO reachable from `async def` (sleep, socket, file, subprocess,
  locks) with prove-only conditions and entry labels
- Site-packages follow down to stdlib sinks; third-party hops marked in JSON
- Third-party rules: `requests`, sync `httpx`, redis, SQLAlchemy, psycopg,
  boto3, sync OpenAI/Anthropic, and more — async twins never fire
- Higher-order / offload: `asyncio.to_thread`, executors, inferred wrappers

## Benchmark

Public cold/warm numbers on pinned asyncio corpora:
[docs/benchmark.md](docs/benchmark.md). Reproduce with `./bench/run.sh`.

## Agent integrations

```bash
npx skills add maolonglong/async-doctor
```

Works once the repo is public. For GitHub code scanning, see `--format sarif` above.

## License

MIT. Bundled typeshed stubs (via `ty_vendored`) are Apache-2.0; see
[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
