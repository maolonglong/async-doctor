# async-doctor

**Your agent blocks the event loop. This catches it.**

Zero-config static analysis for Python: finds sync/blocking IO reachable from
`async def` code, and reports evidence an agent can act on. Inspired by
[react-doctor](https://github.com/millionco/react-doctor); non-goals (no
runtime probing, no Trio-only / gevent-eventlet modelling) live in
[ARCHITECTURE.md](ARCHITECTURE.md).

## Install

```bash
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
async-doctor --python /path/to/.venv .
async-doctor --score .
async-doctor --format json .
async-doctor --format markdown . >> "$GITHUB_STEP_SUMMARY"
async-doctor --format sarif . > async-doctor.sarif

async-doctor explain blocking-sleep
async-doctor why app.py:12 --path .
async-doctor install-skill --path .

# CI / agent loops (full analysis; filtered reporting):
async-doctor --changed .
async-doctor --changed=origin/main .
async-doctor --write-baseline baseline.json .
async-doctor --baseline baseline.json .
```

Every finding includes an evidence chain (`file:line` per hop), a stable rule
id, a `docsAnchor` into [docs/rules.md](docs/rules.md), and a runnable `why`
command. Diagnosis does not require a prescribed repair: `fix` always retains
its stable `fixGroupId`, while an empty `fix.hint` means **no recommended edit**,
not a clean result. Only sync use of a proven async-only context or iteration
protocol inside an already-async function may receive a local correction on
the same object; this is not permission to convert a containing API or a claim
of a globally optimal repair. No config file — flags and inline comments only.

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
  locks, smtplib/ftplib, zip/tar, glob/pathlib walks, `input`, futures waits)
  with prove-only conditions and entry labels
- Site-packages follow down to stdlib sinks; third-party hops marked in JSON
- Third-party rules: `requests`, sync `httpx`, redis, SQLAlchemy, psycopg,
  boto3 client/resource calls, sync OpenAI/Anthropic, and more — async twins
  never fire as findings
- Higher-order / offload: `asyncio.to_thread`, executors, inferred wrappers
  (without hiding their loop-side pre/postamble), typed asyncio callbacks,
  local generator consumption, and project context-manager methods
- **Advice** (`offload-has-async-twin`, `dangling-task`, `ephemeral-async-client`,
  `shared-async-session`, `offload-stateful-session`, `useless-local-async-lock`,
  `swallowed-cancelled-error`): same-package async-symbol availability behind
  offload cuts (not a recommendation to replace the offload or a measured
  performance claim), discarded `create_task` / `ensure_future` handles,
  per-request / per-loop HTTP client pools, shared non-concurrency-safe async DB
  sessions across `gather` / `TaskGroup`, offloaded bound sync DB session/cursor
  methods, per-call unshared `asyncio.Lock` / `Semaphore` guards, and swallowed
  `CancelledError` handlers (non-blocking; SARIF `note`; text **Advice** section,
  capped unless `--expand`)
- **Errors:** `nested-event-loop` (`asyncio.run` / … from async without offload;
  loop-state guards like `if loop is None` / early-return `if loop is not None:
  return` are dropped), `offload-coroutine` (offload target is a coroutine
  function, not an async generator),
  `sync-with-async-cm` (sync `with` on an async-only context manager),
  `sync-for-async-iterable` (sync `for` or comprehension on an async-only
  iterable), `eager-offload-call` (`to_thread(fn(...))` where `fn(...)` runs on
  the loop and returns a non-callable), and `unawaited-coroutine` (discarded
  coroutine calls, or coroutine objects used directly as iteration/context
  protocols)

## GitHub Actions & pre-commit

Use the bundled composite GitHub Action (`action.yml`) for zero-config checks,
step summaries, and optional GitHub Code Scanning SARIF upload:

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
      actions: read
      contents: read
    steps:
      - uses: actions/checkout@v6
        with:
          fetch-depth: 0
      - uses: astral-sh/setup-uv@v7
      - run: uv sync --frozen
      - uses: maolonglong/async-doctor@v0
        with:
          args: "--changed"
          sarif: "true"
```

Or run as a `pre-commit` hook (`.pre-commit-hooks.yaml`; pass `args: [--changed]`
for diff-scoped commits that short-circuit in `<20ms` when no Python or config
files changed):

```yaml
repos:
  - repo: https://github.com/maolonglong/async-doctor
    rev: v0.1.4
    hooks:
      - id: async-doctor
```

## Large repositories / memory

On a 4-core Cloud VM (Linux x86_64), the `litellm/` package subdirectory
(**3009** `.py` files, with venv) cold-scans in about **8.40 s** at
**~1.11 GiB** peak RSS — see [docs/benchmark.md](docs/benchmark.md) for all
corpora. Prefer a package subdirectory (`async-doctor path/to/pkg`) when memory
is tight, and `--max-findings N` when the JSON payload is what matters (litellm
shrinks from ~38 MB to <1 MB at N=50 while score/sinkSites stay full-report).
Warm runs reuse the whole-scan cache when inputs are unchanged.

## Benchmark

Public cold/warm numbers on pinned asyncio corpora:
[docs/benchmark.md](docs/benchmark.md). Reproduce with `./bench/run.sh`.

## Agent integrations

```bash
# Built-in zero-Node installer (writes .agents/skills/async-doctor/SKILL.md + .claude/skills symlink):
async-doctor install-skill --path .

# Or via Node skills CLI:
npx skills add maolonglong/async-doctor
```

For GitHub code scanning, see the SARIF workflow above.

## Development in orbs

Amp runs [`.agents/setup`](.agents/setup) when preparing an orb snapshot. It
installs native build prerequisites, the Rust toolchain and components from
`rust-toolchain.toml`, prebuilt `cargo-nextest`, and an isolated Maturin matching
`pyproject.toml`, then fetches dependencies from `Cargo.lock` without changing it.
Repeated setup reuses installed tools and caches. Compilation stays on demand
so the optimized ty/ruff dependencies do not delay orb startup.

[`.agents/resume`](.agents/resume) installs nothing: this CLI needs no services
or per-thread authentication, and installed tools and caches survive pauses.
The lifecycle scripts take effect for new orbs once pushed to the project's
base branch; an exact snapshot skips setup entirely.

## License

MIT. Bundled typeshed stubs (via `ty_vendored`) are Apache-2.0; see
[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
