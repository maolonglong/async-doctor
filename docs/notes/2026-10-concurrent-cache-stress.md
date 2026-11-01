# Concurrent cold-cache race stress

How to hunt sticky-miss races in the package/scan caches without slowing
default CI.

## Symptom

Intermittent wrong warm/cold results under parallel child processes, or a
stress test that only fails when the sticky-miss window is widened.

## Cause

Cold-cache races between concurrent `async-doctor` children. Debug builds can
pass an internal env to children that widens the sticky-miss window so the race
reproduces reliably; that hook is compiled inert in release.

## Fix or workaround

Default `cargo test` stays fast. Longer loops are `#[ignore]`:

```bash
cargo test -p async_doctor --test concurrent_cache -- --ignored
```

- CI runs a short N≈15–20 variant. Children are spawned in parallel (not
  sequential `.output()`).
- With the fix reverted, the CI-sized large test failed **10/10** debug runs
  (~1s each); with the fix it passed **10/10** (~10.5s each).
- The ignored N=100 stress is for local hunting without relying on the debug
  hook alone.

## Guardrail

`crates/async_doctor/tests/concurrent_cache.rs` (short CI variant +
`#[ignore]` N=100). Command pointer in `AGENTS.md`.
