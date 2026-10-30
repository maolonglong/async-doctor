# async-doctor

**Your agent blocks the event loop. This catches it.**

Zero-config static analysis for Python: finds sync/blocking IO reachable from
`async def` code, and reports evidence an agent can act on.

> Status: **M1** — typed call graph + stdlib sink propagation across the
> project's own modules (shortest evidence chain, offload cuts, entry labels).
> Following into site-packages is M2. See [docs/design.md](docs/design.md).

## Quick start

```bash
# From a built wheel (no Rust toolchain required):
uvx async-doctor .

# Or during development:
cargo run -p async_doctor -- .
cargo run -p async_doctor -- --format json .
cargo run -p async_doctor -- --include-non-production --show-unresolved .
```

Exit codes:

| Code | Meaning |
|------|---------|
| 0 | No findings |
| 1 | Findings present |
| 2 | Tool error |

Incomplete analysis sets `complete=false` in JSON (and warns on stderr); it
does **not** use exit code 2. Candidates never affect the exit code.

By default, entries under `tests/` / `docs/` / `examples/` / `scripts/` /
hidden dirs are skipped (count shown in the summary;
`--include-non-production` to include). `reason=unresolved` candidates are
hidden unless `--show-unresolved`.

## What M1 detects

- Stdlib blocking sinks reachable from `async def` (sleep, socket, urllib,
  subprocess, file IO, threading/queue locks, sqlite3), including multi-hop
  chains across project modules.
- Typed receivers (`self.client.get()`), import aliases, function-local
  imports; shadowed names do not false-match.
- Offload boundaries that cut propagation: `asyncio.to_thread`,
  `loop.run_in_executor`, `anyio.to_thread.run_sync`,
  `starlette.concurrency.run_in_threadpool`, `ThreadPoolExecutor.submit/map`
  (plus `functools.partial` / lambdas passed to those). Eager
  `to_thread(f())` still reports.
- Entry labels: `route` / `lifecycle` / `task` / `other`. FastAPI/Starlette
  sync `def` routes are not treated as entries.
- Prove-only `conditions[]` (`unconditional`/`all-paths` never share a path
  with `if`/`else`/`try`/`match`; `cache_miss_dependent` for module-cache
  checks). Parameter-guarded sinks → `candidates` (`reason=conditional`)
  unless the arg is provably truthy.
- Generators: bare `g = gen()` does not propagate; `for` / `list()` / `next()`
  / `yield from` do. Module-level constant dead branches are pruned.
- `open(...)` + same-entry `f.read()`/`f.write()` merge into one finding.
- Unresolvable dynamic dispatch (`getattr`, …) → `candidates` with reason
  `unresolved` (hidden by default). Small metadata IO → candidates.
- Stable finding `id`s (rule + entry + sink + chain qualnames — not
  line/column). Async-twin names appear only as `fix.hint` text.

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
