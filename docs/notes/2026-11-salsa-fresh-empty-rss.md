# Periodic `fresh_empty()` blew peak RSS on large monorepos

`bce8bb2` periodic Salsa DB rebuilds caused default-mode cold scans to OOM on
large monorepos. Keep one `Storage` per worker on the default path; reserve
`fresh_empty` for batch modes, panic recovery, and follow.

## Symptom

Cold default scans of saleor / dify / litellm / home-assistant core grew until
OOM (multi‑GB → 8–12 GB+) after `bce8bb2`, while `c503648` finished near prior
RSS. `ASYNC_DOCTOR_PASS2_BATCH=locality` bounded memory but was much slower.

## Cause

Salsa `Storage` clones share one `Arc<Zalsa>`. `AsyncDoctorDb::fresh_empty()`
allocates a new independent Zalsa. Retaining retired worker DBs until scan end
kept every rebuilt arena alive, so RSS grew with
`files / reset_interval × workers`.

## Fix or workaround

Remove periodic rebuilds from parallel pass 1 / pass 2. Keep rebuilds for
post-pass-1 release before follow, pass-2 panic recovery,
`ASYNC_DOCTOR_PASS2_BATCH` locality/fixed chunks, and third-party follow. Do
not make locality the default.

## Guardrail

`crates/async_doctor_ty/src/tests/db_resets.rs` asserts `fresh_empty` stays
O(1) on a 400-file default-path scan. Benchmark RSS:
[../benchmark.md](../benchmark.md).
