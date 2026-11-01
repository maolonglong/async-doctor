# react-doctor parity: UX, scoring, skill install, and async anti-patterns

## Purpose

Close the highest-leverage product and analysis gaps relative to `react-doctor` across three milestones:
- **P0:** TTY-aware terminal styling (`NO_COLOR`/`FORCE_COLOR`), scan summary footer (files, async entries, elapsed time), detected async framework versions in the environment banner, and `--format markdown` for GitHub Actions step summaries and PR comments.
- **P1:** Deterministic `0–100` health score (`Report::score` and `--score` CLI flag) plus a built-in `async-doctor install-skill` subcommand for Python-only (`uvx`) environments.
- **P2:** Zero-false-positive detection of common agent-written async Python anti-patterns (`sync-with-async-cm` finding when an async-only context manager is entered via sync `with`, and `dangling-task` correctness advice when `create_task` / `ensure_future` is called as a bare statement without keeping a reference or awaiting).

## Progress

- [x] 2026-11-01T17:17Z: P0 — Terminal UX (TTY color via `anstyle`/`anstream`, scan footer, framework detection in banner) and `--format markdown`.
- [x] 2026-11-01T17:24Z: P1 — Deterministic `0–100` health score (`--score`, `Report::score`) and `async-doctor install-skill`.
- [ ] 2026-11-01: P2 — Async anti-pattern rules (`sync-with-async-cm` finding, `dangling-task` correctness advice).

## Surprises & Discoveries

- `anstyle` (`1.0.14`) and `anstream` (`1.0.0`) were already present in `Cargo.lock` via `clap` and `ruff_*`, so TTY color support and `NO_COLOR`/`CLICOLOR_FORCE` handling required zero new transitive dependencies.

## Decision log

- Decision: keep existing plain-text assertions working when stdout is not a TTY; enable ANSI styling only on interactive terminals or when `FORCE_COLOR` is set (and never when `NO_COLOR` is set) / Date: 2026-11-01 / Rationale: preserves machine-readability in pipes and backwards compatibility for CLI tests.
- Decision: emit `dangling-task` in `advice[]` (`AdviceCategory::Correctness`) and `sync-with-async-cm` in `findings[]` / Date: 2026-11-01 / Rationale: `with <async-only-cm>` raises `TypeError` deterministically at runtime, whereas fire-and-forget `create_task(...)` may be intentional in legacy scripts and belongs in non-blocking correctness advice so existing finding baselines stay stable.

## Context

`async-doctor` already ships the zero-config CLI, call-graph extraction (`crates/async_doctor_ty`), reverse-BFS propagation (`crates/async_doctor_core`), sink/twin tables (`crates/async_doctor_rules`), `--changed`/`--baseline`, SARIF, and `skills/async-doctor/SKILL.md`. This plan adds the remaining developer experience, CI markdown reporting, quantitative scoring, pure-Python skill installation, and async anti-pattern rules inspired by `react-doctor`.

## Plan of work

1. **Milestone P0 (UX & Markdown):**
   - Detect installed async frameworks/libraries from `.dist-info` metadata in `crates/async_doctor_ty/src/env_info.rs` and surface them in `ScanEnvironment::frameworks` and the text environment banner.
   - Track scanned first-party file count and async entry count on `Report` (`file_count`, `async_entry_count`), add TTY-aware ANSI styling (`crates/async_doctor/src/text/style.rs`), and print a scan summary footer with elapsed time.
   - Add `Format::Markdown` (`--format markdown`) in `crates/async_doctor/src/markdown.rs` for GitHub Actions `$GITHUB_STEP_SUMMARY` and PR comments.
2. **Milestone P1 (Health Score & Skill Installer):**
   - Add deterministic `0–100` health score calculation (`Report::score`, `Report::refresh_score`) and `--score` CLI flag.
   - Add `async-doctor install-skill [--path DIR]` (`crates/async_doctor/src/skill_cmd.rs`) writing `.agents/skills/async-doctor/SKILL.md` and `.claude/skills/async-doctor` symlink.
3. **Milestone P2 (Async Anti-Patterns):**
   - Add `sync-with-async-cm` finding rule when a sync `with` statement enters an async-only context manager (e.g. `httpx.AsyncClient`, `aiohttp.ClientSession`, `asyncio.Lock`, `asyncio.TaskGroup`, or a project class defining `__aenter__` without `__enter__`).
   - Add `dangling-task` correctness advice rule when `asyncio.create_task` / `loop.create_task` / `asyncio.ensure_future` is called as a bare expression statement without awaiting or storing the returned `Task`.

## Validation

- P0: `async-doctor --format markdown fixtures/m1_multihop` emits GFM tables and collapsible chains; text output includes `Scanned ... file(s)` footer and detected frameworks when present; `FORCE_COLOR=1` emits ANSI codes while `NO_COLOR=1` suppresses them.
- P1: `async-doctor --score` prints an integer in `0..=100` (`100` on `fixtures/negative_sync.py`, `< 100` on `fixtures/m1_multihop`); `async-doctor install-skill --path <tmp>` creates `.agents/skills/async-doctor/SKILL.md` and `.claude/skills/async-doctor`.
- P2: `sync-with-async-cm` reports a finding on `with httpx.AsyncClient():` or a project async-only context manager, and stays silent on `async with` or dual sync+async context managers; `dangling-task` emits `correctness` advice on bare `asyncio.create_task(coro())` and stays silent when assigned (`t = asyncio.create_task(...)`), returned, or passed as an argument.
- All milestones: `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, `./scripts/check_rust_file_length.sh`, and `python3 scripts/check_docs.py` pass.

## Outcomes & Retrospective

(Active — fill on completion.)
