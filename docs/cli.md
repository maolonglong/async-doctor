# CLI reference

User-facing commands and flags. Exact option text comes from `--help`; this
page is the behavioural contract agents should preserve. JSON shape:
[json-contract.md](json-contract.md). Rules: [rules.md](rules.md).

## Commands

```bash
async-doctor [OPTIONS] [PATH]                        # scan project dir or single .py file
async-doctor explain <rule-id>                       # long-form rule help + docs/rules.md anchor
async-doctor why file:line --path DIR [--format json]  # re-scan; explain findings/candidates/advice at a site
async-doctor install-skill [--force] --path DIR      # install .agents/skills/async-doctor + .claude/skills symlink
```

## Reporting and filters

| Flag / behaviour | Contract |
| --- | --- |
| Default format | Human text (environment + sink-site summary, then findings / advice + scan summary footer with 0–100 score; TTY-styled via `anstyle`/`anstream`, respects `NO_COLOR` and `CLICOLOR_FORCE`/`FORCE_COLOR`) |
| `--format json` | Schema in [json-contract.md](json-contract.md) |
| `--format sarif` | SARIF 2.1.0 for code scanning (canonical `driver.rules` + `ruleIndex` + RFC 3986 percent-encoded URIs) |
| `--format markdown` | GitHub-Flavored Markdown for `$GITHUB_STEP_SUMMARY` and PR comments |
| `--score` | Print only the 0–100 health score on stdout (exit code still reflects findings unless `--fail-on none`) |
| `--fail-on <findings\|none>` | Exit-code policy: `findings` (default) exits 1 on findings or unused suppressions; `none` exits 0 for advisory CI / preview runs (tool errors still exit 2) |
| `--expand` | Show every finding chain and full advice list (text groups findings and caps advice by default) |
| `--show-unresolved` | Surface `reason=unresolved` candidates (else hidden / aggregated) |
| `--include-non-production` | Include entries under tests/docs/examples/scripts/hidden dirs |
| `--changed[=base]` / `--diff[=base]` | Full analysis; report only findings/candidates/advice/unused-suppressions touching NEW-side changed lines (from `git diff -U0` vs base, unstaged and cached diffs, and untracked files). Limitations: an edit that changes resolution without touching a chain line (a type annotation elsewhere, a pure file rename) is not reported — run a full scan or `--baseline` before merge |
| `--baseline` / `--write-baseline` | CI / agent loops; `--write-baseline` cannot combine with `--changed` / `--diff` |
| `--max-findings N` | Cap `findings`/`candidates`/`advice` each to N (output-only; score, exit code, `sinkSites` and `--write-baseline` use the full report). Findings are picked round-robin over `sinkSites` so every site keeps an example while N >= site count; grouped text, Markdown, and the `omitted*` JSON fields preserve full pre-cap `sinkSites` counts |

## Suppressions

Inline only (zero-config): `# async-doctor: ignore[<rule-id>]: <reason>`.
Reason is required. Supports same-line comments, standalone comments on the
preceding line (including above stacked `@decorator` lines on an `async def`),
and trailing comments on the closing line of a multi-line call. `#` inside
single- or multi-line string literals (`'''` / `"""`) is ignored. Unused
suppressions fail the run (exit 1 unless `--fail-on none`). On single-file
scans (`async-doctor file.py`), unused suppressions are checked strictly in
that target `.py` file. Same syntax covers advice rules.

## Environment

Activate the project venv or set `VIRTUAL_ENV` so third-party imports resolve.
Without a project environment, results degrade and `environment.note` may
explain why. Optional debug/perf env vars (`ASYNC_DOCTOR_TIMING`,
`ASYNC_DOCTOR_PASS2_BATCH`) are documented in [benchmark.md](benchmark.md).

## Agent install

```bash
async-doctor install-skill --path .
async-doctor install-skill --force --path .   # overwrite a divergent SKILL.md / stale symlink
# or via Node skills CLI:
npx skills add maolonglong/async-doctor
```

Writes `.agents/skills/async-doctor/SKILL.md` and links
`.claude/skills/async-doctor` → `../../.agents/skills/async-doctor` (Unix). On
Windows the Claude path is a directory copy of `SKILL.md`. If `SKILL.md`
already exists and differs from the bundled copy, or if the Claude path exists
and is not the expected link/copy, the command skips and prints a clear
message; pass `--force` to overwrite the skill file and replace a stale
symlink/file, or to refresh a managed Windows directory copy in place. A real
directory at `.claude/skills/async-doctor` that contains user files is never
deleted; move it aside and re-run to create the symlink.

Product skill path: `skills/async-doctor/SKILL.md`. No editor stop hook
([decisions/2026-10-31-no-stop-hook.md](decisions/2026-10-31-no-stop-hook.md)).
