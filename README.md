# async-doctor

**Your agent blocks the event loop. This catches it.**

Zero-config static analysis for Python: finds sync/blocking IO reachable from
`async def` code, and reports evidence an agent can act on.

> Status: **M3** — agent-facing explain/why, `--changed` / `--baseline`, inline
> suppressions, SARIF 2.1.0, and integrations. Analysis core remains M2. See
> [docs/design.md](docs/design.md).

## Quick start

```bash
# From a built wheel (no Rust toolchain required):
uvx async-doctor .

# Or during development:
cargo run -p async_doctor -- .
cargo run -p async_doctor -- --format json .
cargo run -p async_doctor -- --format sarif . > async-doctor.sarif

# Long-form rule help / site evidence:
cargo run -p async_doctor -- explain blocking-sleep
cargo run -p async_doctor -- why helpers.py:5 --path fixtures/m1_multihop

# CI / agent loops (full analysis; filtered reporting):
cargo run -p async_doctor -- --changed .
cargo run -p async_doctor -- --changed=origin/main .
cargo run -p async_doctor -- --write-baseline baseline.json .
cargo run -p async_doctor -- --baseline baseline.json .
```

Every finding includes the full call chain (`file:line` per hop), a concrete fix
hint, a stable rule id, a `docsAnchor` into [docs/rules.md](docs/rules.md), and
a runnable `why` command. No config file — flags and inline comments only.

Inline suppressions (reason required):

```python
time.sleep(1)  # async-doctor: ignore[blocking-sleep]: brief sync boundary in CLI wrapper
```

Unused ignores fail the run.

### Exit codes

| Code | Meaning |
|------|---------|
| 0 | Clean — no findings (or no *new* findings vs `--baseline`), and no unused suppressions |
| 1 | Findings present (or unused suppressions) |
| 2 | Tool error |

Incomplete analysis sets `complete=false` in JSON (and warns on stderr); it
does **not** use exit code 2. Candidates never affect the exit code. Baseline
fingerprints use stable finding ids (rule + symbol path + sink), not line numbers.

By default, entries under `tests/` / `docs/` / `examples/` / `scripts/` /
hidden dirs are skipped (`--include-non-production` to include).
`reason=unresolved` candidates are hidden unless `--show-unresolved`.

## Agent integrations

Ship-ready snippets under [`integrations/`](integrations/):

- [`integrations/skill/SKILL.md`](integrations/skill/SKILL.md) — teach coding
  agents when/how to run async-doctor and fix findings
- [`integrations/stop-hook/`](integrations/stop-hook/) — Claude Code / Cursor
  stop-hook examples that run `async-doctor --changed` and block completion on
  new findings

## What it detects (M2 analysis)

- **Site-packages follow** down to stdlib sinks; third-party hops marked in JSON
- **Third-party rules:** `requests`, sync `httpx`, redis, SQLAlchemy, psycopg,
  boto3, sync OpenAI/Anthropic, and more — async twins never fire
- **Higher-order / offload:** `asyncio.to_thread`, executors, inferred wrappers
- Stdlib sinks (sleep, socket, file, subprocess, locks), prove-only conditions,
  entry labels, stable finding ids

## Packaging

Wheels are built with [maturin](https://www.maturin.rs/) (`bindings = "bin"`),
so `uvx` / `uv run` install a native `async-doctor` binary.

```bash
maturin build --release
uvx --from ./target/wheels/async_doctor-*.whl async-doctor --help
```

## License

MIT. Bundled typeshed stubs (via `ty_vendored`) are Apache-2.0; see
[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
