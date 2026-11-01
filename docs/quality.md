# Quality and tech debt

Grades show where the codebase is solid and where agents should tread carefully.
The debt tracker lists known shortcuts so they get paid down in small steps
instead of piling up.

## Grades

| Area | Grade | Why | Updated |
|---|---|---|---|
| `async_doctor` CLI / reporters | B | Solid CLI tests; SARIF/text paths less exercised than JSON | 2026-11-01 |
| `async_doctor_ty` extract / scan / cache | B | Broad fixtures; pass-2 and cache modules over the 500-line target | 2026-11-01 |
| `async_doctor_core` propagate / report | B | Prove-only core works; `propagate/mod.rs` oversized; memory on huge repos | 2026-11-01 |
| `async_doctor_rules` sinks / twins | B | Good family coverage; P1 stdlib/boto recall gaps closed | 2026-11-01 |
| Docs / agent map | B | Bootstrapped 2026-11-01; gardening is on-demand via audit, not scheduled | 2026-11-01 |

## Debt tracker

| Debt | Where | Cost of leaving it | Added |
|---|---|---|---|
| `propagate/mod.rs` ~634 lines (over 500 target) | `crates/async_doctor_core/src/propagate/mod.rs` | Harder reviews; split-by-responsibility risk | 2026-11-01 |
| `cache.rs` ~515 lines (over 500 target) | `crates/async_doctor_ty/src/cache.rs` | Same | 2026-11-01 |
| `CallCollector` constructor duplicated 3× | `pass2.rs` (×2), `follow.rs` | Drift between extract paths | 2026-11-01 |
| Memory on very large repos (Home Assistant core OOM above ~6GB) | extract / pass-2 | Cannot scan some monorepos whole | 2026-11-01 |
| GBK / legacy-encoded sources skipped with a warning | `encoding.rs` | Those files invisible to analysis | 2026-11-01 |
| Serial pass-2 panic rollback does not undo changes to existing functions / `follow_targets` (deliberate) | `pass2.rs` | Partial facts after panic; document-only today | 2026-11-01 |
| Vendored `fast-rust` has no upstream license | `.agents/skills/fast-rust/` | Must remove or license before open-sourcing | 2026-11-01 |
