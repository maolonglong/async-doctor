# M3 — Agent integration

## Purpose

Make async-doctor usable in agent/CI loops: `why`, `--changed`, `--baseline`,
inline ignore, SARIF, and a thin installable skill — without editor stop hooks.

## Progress

- [x] `async-doctor why file:line` and `explain <rule-id>` with `docs/rules.md` anchors.
- [x] `--changed [base]` and `--baseline` / `--write-baseline`.
- [x] Inline `# async-doctor: ignore[rule]: <reason>`; unused ignores fail the run.
- [x] SARIF 2.1.0 reporter.
- [x] M3a precision corpus fixes ([2026-10-m3a-precision.md](2026-10-m3a-precision.md), [2026-10-m3a-higher-order.md](2026-10-m3a-higher-order.md)).
- [x] Advice tier + nested-event-loop / offload-coroutine (https://github.com/maolonglong/async-doctor/pull/30).
- [x] Skill at `skills/async-doctor/SKILL.md`; stop-hook removed (https://github.com/maolonglong/async-doctor/pull/29).
- [x] schemaVersion 2 / environment reporting / sink-first summary (https://github.com/maolonglong/async-doctor/pull/32).

## Surprises & Discoveries

- Stop-hook packaging added little versus `npx skills add`; removed to match react-doctor.
- Advice must fire only on already-cut offloads to avoid double-reporting with `blocking-*`.

## Decision log

- Decision: no stop hook / Date: 2026-10-31 / Link: [../../decisions/2026-10-31-no-stop-hook.md](../../decisions/2026-10-31-no-stop-hook.md).
- Decision: advice tier / Date: 2026-10-31 / Link: [../../decisions/2026-10-31-advice-tier.md](../../decisions/2026-10-31-advice-tier.md).

## Validation

- CLI tests under `crates/async_doctor/tests/` (`cli_explain`, `cli_changed_baseline`, `cli_sarif`, `cli_json_contract`, `cli_suppress_gitignore`).

## Outcomes & Retrospective

Agent loops work via CLI + skill + SARIF. MCP deferred until after first
release evaluation. Remaining recall/debt in [../../quality.md](../../quality.md).
