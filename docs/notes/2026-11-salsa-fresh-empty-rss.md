# Uncoordinated per-worker `fresh_empty()` and retained ASTs blew peak RSS on large monorepos

`bce8bb2` per-worker Salsa DB rebuilds retained every retired `Arc<Zalsa>` until
scan end (`workers × resets` live arenas), and Salsa's `lru=200` on
`parsed_module` only evicts across revisions (`reset_for_new_revision`), never
within a single revision. Bound RSS with explicit `ParsedModule::clear()` and
coordinated multi-worker waves that drop the shared `Arc<Zalsa>` to refcount 0
between waves.

## Symptom

Cold default scans of saleor / dify / litellm / home-assistant core grew to
multi‑GB (or 8–12 GB+ under `bce8bb2`). Even with a single shared `Arc<Zalsa>`,
a 4,000-file repository accumulated ~680 MiB of `parsed_module` ASTs and ~2.8 GiB
of total Salsa query/struct memory by the end of Pass 2.

## Cause

1. Salsa `Storage` clones share one `Arc<Zalsa>`, whereas
   `AsyncDoctorDb::fresh_empty()` allocates a new independent `Zalsa`. Retaining
   retired worker DBs in `bce8bb2` kept every rebuilt arena alive simultaneously.
2. `ruff_db::parsed::parsed_module` configures `lru=200`, which Salsa only
   applies during revision bumps (`reset_for_new_revision`). Within a single
   immutable scan revision, every first-party and third-party `ParsedModule`
   stayed resident unless explicitly cleared via `ParsedModule::clear()`.
3. Uncapped thread counts multiplied `ZalsaLocal` page allocations and allocator
   per-thread magazines without improving wall time past 8 threads.

## Fix or workaround

1. Clear each file's `ParsedModule` AST immediately after Pass 1 (`functions.rs`)
   and Pass 2 (`pass2.rs`) extraction, and periodically clear all read paths'
   `ParsedModule`s every 25 files per worker (`clear_all_parsed_modules`) so
   transitive third-party and typeshed ASTs do not accumulate.
2. Disable diagnostic-only rule selection and `type: ignore` comment scanning on
   `AsyncDoctorDb` (`should_check_file() -> false`,
   `respect_type_ignore_comments: false`).
3. Cap default Pass 1 / Pass 2 worker threads at 8 (`DEFAULT_MAX_PASS_THREADS`).
4. Run Pass 2 in coordinated 1,000-file waves (`PASS2_WAVE_FILES = 1000`):
   all workers share one `Arc<Zalsa>` within a wave, join at the wave boundary,
   drop all worker DB clones, and replace `base_db` with `fresh_empty()` +
   `release_freed_memory()` before the next wave starts. At most one `Arc<Zalsa>`
   exists at any time, and repositories with `<= 1,000` files incur zero
   intermediate wave resets.

## Guardrail

`crates/async_doctor_ty/src/tests/db_resets.rs` asserts `fresh_empty` stays
bounded on both 400-file single-wave scans (`1..=2`) and 1,050-file multi-wave
scans (`2..=4`, while verifying cross-wave call resolution). Benchmark RSS:
[../benchmark.md](../benchmark.md).

## 2026-11-02 follow-up: wave size 1,000 → 2,500

Each extra wave re-infers shared heavy modules. Cold scans on public repos
(macOS, no venv, one run each; wall s / peak RSS GiB):

| repo (files) | waves 1000 | waves 2500 | waves off |
| --- | --- | --- | --- |
| litellm (2399) | 5.79 / 1.00 | 5.38 / 1.43 | 4.81 / 1.59 |
| saleor (2645) | 3.25 / 0.37 | 2.04 / 0.57 | 2.05 / 0.75 |
| dify `api` (3945) | 7.32 / 1.01 | 4.58 / 1.43 | 4.43 / 1.62 |
| HA core (9265) | 12.36 / 0.67 | 6.83 / 0.82 | 7.24 / 2.04 |

JSON was byte-identical across all three settings. `PASS2_WAVE_FILES` is now
2,500. A memory-triggered wave (roll only past an RSS / Salsa memo threshold)
is still open; validate it on a Linux box with project venvs, where the
first-release audit saw larger slowdowns (saleor 15-20 s → 42-50 s).
