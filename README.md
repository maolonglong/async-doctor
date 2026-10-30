# async-doctor

**Your agent blocks the event loop. This catches it.**

Zero-config static analysis for Python: finds sync/blocking IO reachable from
`async def` code, and reports evidence an agent can act on.

> Status: **M3** (in progress) — agent-facing explain/why, then `--changed` /
> baseline / SARIF / integrations. Analysis core remains M2. See
> [docs/design.md](docs/design.md).

## Quick start

```bash
# From a built wheel (no Rust toolchain required):
uvx async-doctor .

# Or during development:
cargo run -p async_doctor -- .
cargo run -p async_doctor -- --format json .
cargo run -p async_doctor -- --include-non-production --show-unresolved .

# Long-form rule help / site evidence (agent-facing):
cargo run -p async_doctor -- explain blocking-sleep
cargo run -p async_doctor -- why helpers.py:5 --path fixtures/m1_multihop

# CI / agent loops (analysis is always whole-project; reporting is filtered):
cargo run -p async_doctor -- --changed .
cargo run -p async_doctor -- --changed=origin/main .
cargo run -p async_doctor -- --write-baseline baseline.json .
cargo run -p async_doctor -- --baseline baseline.json .
```

Every finding includes the full call chain (file:line per hop), a concrete fix
hint, a stable rule id, a `docsAnchor` into [docs/rules.md](docs/rules.md), and
a runnable `why` command.

Inline suppressions (reason required):
`# async-doctor: ignore[blocking-sleep]: <why this is safe>`. Unused ignores fail
the run.

Exit codes:

| Code | Meaning |
|------|---------|
| 0 | Clean — no findings (or no *new* findings vs `--baseline`), and no unused suppressions |
| 1 | Findings present (or unused suppressions) |
| 2 | Tool error |

Incomplete analysis sets `complete=false` in JSON (and warns on stderr); it
does **not** use exit code 2. Candidates never affect the exit code. Baseline
fingerprints use stable finding ids (rule + symbol path + sink), not line numbers.

By default, entries under `tests/` / `docs/` / `examples/` / `scripts/` /
hidden dirs are skipped (count shown in the summary;
`--include-non-production` to include). `reason=unresolved` candidates are
hidden unless `--show-unresolved` (which also prints an aggregated pattern
summary ranked by entries affected).

## What M2 detects (includes M1)

- **Site-packages follow:** continues into reachable third-party `.py` down to
  stdlib sinks; only functions reachable from project `async def` entries.
  Per-`(package, version)` cache under `$XDG_CACHE_HOME/async-doctor`.
  Third-party chain hops are marked in JSON (`thirdParty` / `package`).
- **Third-party rules** (type-based; also C extensions without source):
  `requests`, sync `httpx.Client`, `urllib3`, sync `redis`, sync SQLAlchemy
  `Session`/`Engine` (`Engine.begin` only in `with`), `psycopg`/`psycopg2`,
  `pymysql`, `pymongo`, `boto3`/`botocore`, sync `openai`/`anthropic`,
  `kafka-python`, sync Elasticsearch, sync gRPC. Async counterparts never fire;
  fix hints name the async twin.
- **Higher-order args** and **ty-resolved offload** (not bare name), plus
  inferred wrappers whose bodies call a real offload primitive.
- Nested defs keyed by full scope; lambda assign+call; blocking `@property`;
  genexprs consumed by `next`/`list`; imperative FastAPI/Starlette routes.
- Stdlib sinks, prove-only conditions, entry labels, generator consume rules,
  and stable finding ids from M1 are unchanged.

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
