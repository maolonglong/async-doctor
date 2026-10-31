---
name: async-doctor
description: >-
  Use after changing async Python code or before committing. Run
  `async-doctor --changed` for a regression check; run `async-doctor .` for a
  full audit. Detects sync/blocking IO reachable from async def (sleep, HTTP,
  DB, file, subprocess, locks). Use explain/why to fix findings.
version: "1.0.0"
---

# async-doctor

Your agent blocks the event loop. This catches it.

Zero-config static analysis for Python: finds sync/blocking IO reachable from
`async def`, with a full evidence chain and a concrete fix hint.

## After editing async Python

Run from the project root:

```bash
async-doctor --changed
# or, during development:
cargo run -p async_doctor -- --changed .
```

Fix every finding before finishing. Candidates do not fail the exit code; findings do.

| Exit | Meaning |
| ---- | ------- |
| 0 | Clean (no findings / no new findings vs baseline) |
| 1 | Findings (or unused suppressions) |
| 2 | Tool error |

## For a full cleanup

```bash
async-doctor .
async-doctor --format json .
```

## Understanding a finding

Each finding includes:

- **chain** — async entry → … → blocking sink, with `file:line` per hop
- **fix.hint** — await the async twin, `asyncio.to_thread(f, args)`, or switch client
- **rule** / **docsAnchor** — stable id into `docs/rules.md`
- **why** — runnable locator command

```bash
async-doctor explain blocking-sleep
async-doctor why path/to/file.py:42 --path .
```

## Suppressions (no config file)

Only when a sync call is intentionally on the loop thread — reason required:

```python
time.sleep(1)  # async-doctor: ignore[blocking-sleep]: brief sync boundary in CLI wrapper
```

Unused suppressions fail the run. Prefer fixing over ignoring.

## CI / regression

```bash
async-doctor --write-baseline baseline.json .
async-doctor --baseline baseline.json --changed
```

Fingerprints are stable across line shifts (rule + symbol path + sink, not lines).

## SARIF (GitHub code scanning)

```bash
async-doctor --format sarif . > async-doctor.sarif
```
