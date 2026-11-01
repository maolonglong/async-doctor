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

Activate the project venv (or set `VIRTUAL_ENV`) before scanning so third-party
imports resolve. Without a project environment, results silently degrade —
`async-doctor` reports the interpreter/venv it used and prints a short note.

## Quick start

```bash
async-doctor .
async-doctor --format json .
async-doctor --format sarif . > async-doctor.sarif

async-doctor explain blocking-sleep
async-doctor why app.py:12 --path .

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
does **not** use exit 2. Candidates and **advice** never affect the exit code.

By default, entries under `tests/` / `docs/` / `examples/` / `scripts/` /
hidden dirs are skipped (`--include-non-production` to include).
`reason=unresolved` candidates are hidden unless `--show-unresolved`.

## What it detects

- Sync/blocking IO reachable from `async def` (sleep, socket, file, subprocess,
  locks) with prove-only conditions and entry labels
- Site-packages follow down to stdlib sinks; third-party hops marked in JSON
- Third-party rules: `requests`, sync `httpx`, redis, SQLAlchemy, psycopg,
  boto3, sync OpenAI/Anthropic, and more — async twins never fire as findings
- Higher-order / offload: `asyncio.to_thread`, executors, inferred wrappers
- **Advice** (`offload-has-async-twin`): offload cuts that only reach sinks with
  a high-confidence same-package async twin installed in the scanned venv
  (non-blocking; SARIF `note`; text **Advice** section, capped unless `--expand`)
- **Errors:** `nested-event-loop` (`asyncio.run` / … from async without offload;
  loop-state guards like `if loop is None` are dropped), `offload-coroutine`
  (offload target is `async def`)

## GitHub Actions (SARIF / code scanning)

Upload SARIF so findings appear in GitHub code scanning:

```yaml
name: async-doctor
on:
  push:
    branches: [main]
  pull_request:

jobs:
  scan:
    runs-on: ubuntu-latest
    permissions:
      security-events: write
      contents: read
    steps:
      - uses: actions/checkout@v4
      - uses: astral-sh/setup-uv@v4
      - name: Install project deps
        run: uv sync --frozen
      - name: Run async-doctor
        run: |
          source .venv/bin/activate
          uvx async-doctor --format sarif . > async-doctor.sarif
      - uses: github/codeql-action/upload-sarif@v3
        with:
          sarif_file: async-doctor.sarif
```

## Large repositories / memory

Very large monorepos can use substantial peak RSS (for example, a full litellm
tree peaked around **~6.9 GB** in our benchmarks). Prefer scanning the package
subdirectory you care about (`async-doctor path/to/pkg`) rather than the whole
repo when memory is tight. Warm runs reuse the whole-scan cache when inputs are
unchanged.

## Benchmark

Public cold/warm numbers on pinned asyncio corpora:
[docs/benchmark.md](docs/benchmark.md). Reproduce with `./bench/run.sh`.

## Agent integrations

```bash
npx skills add maolonglong/async-doctor
```

Works once the repo is public. For GitHub code scanning, see the SARIF workflow
above.

## License

MIT. Bundled typeshed stubs (via `ty_vendored`) are Apache-2.0; see
[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
