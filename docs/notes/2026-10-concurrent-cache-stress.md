# Concurrent cold-cache race stress

How to hunt sticky-miss races in the package/scan caches without slowing
default CI / PR checks.

## Symptom

Intermittent wrong warm/cold results under parallel child processes, or a
stress test that only fails when the sticky-miss window is widened.

## Cause

Cold-cache races between concurrent `async-doctor` children. Debug builds can
pass an internal env to children that widens the sticky-miss window so the race
reproduces reliably; that hook is compiled inert in release.

## Fix or workaround

Default `cargo test --workspace` / `cargo nextest run --workspace` and the PR
`test` job stay fast: all `concurrent_cold_scans*` tests are `#[ignore]`.
The N=15/20 variants run on every **main** push (and `workflow_dispatch` /
weekly schedule) in the `stress` job via `--run-ignored only`. Longer N=100
loops stay local-only:

```bash
# Main-sized CI variants (N=15/20), same filter as the stress job:
cargo nextest run -p async_doctor --test concurrent_cache --profile stress \
  --run-ignored only \
  -E 'test(concurrent_cold_scans) and not test(/_stress$/)'

# Local N=100 hunting (also picks up the N=15/20 ignores):
cargo test -p async_doctor --test concurrent_cache -- --ignored
# or:
cargo nextest run -p async_doctor --test concurrent_cache --run-ignored only
```

- Children are spawned in parallel (not sequential `.output()`).
- With the fix reverted, the CI-sized large test failed **10/10** debug runs
  (~1s each); with the fix it passed **10/10** (~10.5s each on a laptop;
  several minutes on Actions before dep opt-level).
- The ignored N=100 stress is for local hunting without relying on the debug
  hook alone.
- nextest puts `concurrent_cold_scans*` in a serial test-group
  (`.config/nextest.toml`) so they do not overlap with other heavy scans.

## Guardrail

`crates/async_doctor/tests/concurrent_cache.rs` (main `stress` job N=15/20 +
`#[ignore]` N=100). Command pointers in `AGENTS.md` and `docs/quality.md`.
