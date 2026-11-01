# CLI reference

User-facing commands and flags. Exact option text comes from `--help`; this
page is the behavioural contract agents should preserve. JSON shape:
[json-contract.md](json-contract.md). Rules: [rules.md](rules.md).

## Commands

```bash
async-doctor [OPTIONS] [PATH]          # scan project dir or single .py file
async-doctor explain <rule-id>         # long-form rule help + docs/rules.md anchor
async-doctor why file:line --path DIR  # re-scan; explain findings/advice at a site
```

## Reporting and filters

| Flag / behaviour | Contract |
| --- | --- |
| Default format | Human text (environment + sink-site summary, then findings / advice + scan summary footer; TTY-styled via `anstyle`/`anstream`, respects `NO_COLOR` and `CLICOLOR_FORCE`/`FORCE_COLOR`) |
| `--format json` | Schema in [json-contract.md](json-contract.md) |
| `--format sarif` | SARIF 2.1.0 for code scanning |
| `--format markdown` | GitHub-Flavored Markdown for `$GITHUB_STEP_SUMMARY` and PR comments |
| `--expand` | Show full advice list (text caps by default) |
| `--show-unresolved` | Surface `reason=unresolved` candidates (else hidden / aggregated) |
| `--include-non-production` | Include entries under tests/docs/examples/scripts/hidden dirs |
| `--changed [base]` | Full analysis; filter reported findings to the git changed set |
| `--baseline` / `--write-baseline` | CI / agent loops; `--write-baseline` cannot combine with `--changed` |

## Suppressions

Inline only (zero-config): `# async-doctor: ignore[<rule-id>]: <reason>`.
Reason is required. Unused suppressions fail the run (exit 1). Same syntax
covers advice rules.

## Environment

Activate the project venv or set `VIRTUAL_ENV` so third-party imports resolve.
Without a project environment, results degrade and `environment.note` may
explain why. Optional debug/perf env vars (`ASYNC_DOCTOR_TIMING`,
`ASYNC_DOCTOR_PASS2_BATCH`) are documented in [benchmark.md](benchmark.md).

## Agent install

```bash
npx skills add maolonglong/async-doctor
```

Product skill path: `skills/async-doctor/SKILL.md`. No editor stop hook
([decisions/2026-10-31-no-stop-hook.md](decisions/2026-10-31-no-stop-hook.md)).
