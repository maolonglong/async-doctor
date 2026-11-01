---
name: agent-docs-workflow
description: >-
  Bootstrap, audit, or garden a repo's agent docs (AGENTS.md map,
  ARCHITECTURE.md, docs/ plans, decisions, notes, quality, check_docs.py). Use
  when setting up docs in a new or existing repo, when docs drift from code,
  after a large merge, or before a release.
---
# Agent docs workflow

Repository docs are the system of record for agents: what is not in the repo does not exist for the next run. This skill installs and maintains one layout: a short `AGENTS.md` **map**, a matklad-style `ARCHITECTURE.md`, and `docs/` with **one home per fact**. The files to copy are in [template/](template/) beside this skill; [template/docs/README.md](template/docs/README.md) holds the placement table and [template/docs/workflow.md](template/docs/workflow.md) the task flow. [references/leakage.md](references/leakage.md) holds the process-leakage probes. After editing `template/scripts/check_docs.py`, run `python3 tests/test_check_docs.py`.

Pick the mode from the request:

- **Bootstrap**: the repo has no `docs/README.md` placement table or no `scripts/check_docs.py`.
- **Audit**: the user asks whether docs are healthy, or before a release.
- **Garden**: run by hand after a large merge or before a release, when the user asks for fixes. Audit, then fix. Nothing schedules this mode.

## Bootstrap

1. **Inventory.** List every doc-like file: root `*.md`, `docs/**`, nested `AGENTS.md`/`CLAUDE.md`, `.cursor/rules`, `.agents/skills`, `.cursor/skills`, design pages linked from README. Done when each file has a planned destination from the placement table.
2. **Copy the template** into the repo root without overwriting existing files. Done when every template path exists in the repo.
3. **Rewrite `AGENTS.md` as a map.** Keep the project's existing standing orders that change agent behaviour; move procedures, test recipes, and stories into `docs/` or `docs/notes/` and leave a one-line pointer. Phrase each order as the target behaviour. Done when it is under 100 lines, every pointer names its trigger, and the diff loses no proposition from the old file.
4. **Write `ARCHITECTURE.md`** from the code, not from old prose: run `tree`/`ls` on the source roots, read entry points and module headers. Fill bird's-eye view, entry points, code map, invariants, boundaries, cross-cutting concerns. Name paths and types in backticks; no links into source. Done when every top-level source directory appears in the code map and every backticked path exists.
5. **Split mixed design docs.** Dated choices with alternatives become `docs/decisions/<date>-<slug>.md` (status `accepted`, alternatives only if recorded; steps and acceptance criteria go to the plan, not the decision). Milestone plans and result logs become `docs/plans/completed/`. Rules for users (rule catalogue, CLI reference) stay as topic docs listed in `docs/README.md`. Before each split, list the source's propositions (see [Writing rules](#writing-rules)). Done when every source file has a destination and the diff loses no proposition: each one appears at its new home or is deliberately dropped and named in the report.
6. **Bring outside knowledge in.** Ask the user, or read the PRs, issues, and threads they point to, for: rejected approaches (decision with status `rejected: <reason>`), open follow-ups (`quality.md` debt rows), in-flight work (an active plan). Record only facts you found; mark gaps as open questions in your report.
7. **Wire the check.** Run `python3 scripts/check_docs.py`; fix every error. Add the CI step from `docs/workflow.md`. Add `ln -s AGENTS.md CLAUDE.md` so Claude Code reads the same map. Put repo agent skills in `.agents/skills/<name>/` (move any from `.cursor/skills/`); Claude Code reads only `.claude/skills/`, so for each skill add `ln -s ../../.agents/skills/<name> .claude/skills/<name>` (the check fails on a missing or wrong link). If the repo has no PR template, copy `template/.github/pull_request_template.md`.
8. **Report.** Done when the check passes, and you list each moved file (old → new), each fact you could not place, and each open question.

## Audit

Run every item; report each as pass, or fail with file and line.

- `python3 scripts/check_docs.py` output (errors and warnings).
- `AGENTS.md`: each line either a standing order most tasks need or a pointer with a trigger. Flag no-ops (rules the agent follows by default), duplicates of a linked doc, and prohibitions with no positive target.
- `ARCHITECTURE.md` against the code: modules missing from the code map, modules in the map that moved, invariants the code now violates (grep for the forbidden dependency), stale entry points.
- Active plans: Progress matches the branch and merged PRs; a plan whose goal shipped moves to `completed/`.
- Decisions: an accepted decision whose code was reversed needs a superseding record. Two live records on the same mechanism mean the older one should be `superseded by` the newer.
- Process leakage: run the probes in [references/leakage.md](references/leakage.md) over docs, comments, test names, and file names. Judge each hit by hand against its false-positive list; report real hits with a rewrite that describes current behaviour, not the change that produced it.
- Facts: re-run each command a doc shows and compare the output; re-measure or trace each number (counts, timings, sizes) to its source. A number you cannot re-run must say how and when it was measured ("measured: 91 s on litellm, 2026-11-01"); otherwise it fails.
- Notes: a lesson that repeats, or a note with "Guardrail: none yet", is a candidate for a test or lint. A note about code that no longer exists is deleted.
- `quality.md`: grades still match reality; paid-off debt rows removed.
- Knowledge outside the repo that agents needed this week (from PR comments, review threads, failed runs): list it with a proposed home.

Done when every item has a verdict and each fail has a proposed fix.

## Garden

1. Run the audit.
2. Fix mechanical failures directly (links, paths, moved plans, stale grades), one small PR per theme, each reviewable in a minute.
3. For a rule that keeps being broken, add it to `check_docs.py` or the project's linters, with an error message that says how to fix it, then shorten the prose that stated it.
4. Leave judgement calls (reversing a decision, changing a standing order) as proposals in the report for the user.

Done when the check passes on the PR branch and the report lists what changed and what waits for a decision.

## Writing rules

These apply to every doc this skill touches.

- Current state only; history lives in git, completed plans, and decisions.
- One home per fact; elsewhere, link.
- Leave lookups to the environment: do not copy what `--help`, a manifest, or a generated file states.
- Lead pointers with the trigger word; one pointer per distinct case.
- Prefer a check over a sentence: a rule that can be linted belongs in the linter.
- **Keep every proposition.** Before deleting, moving, or condensing text, list what it says: subjects and actions, conditions and timing, must/may/never, exceptions, and what fails if it is ignored. After the edit, check each one survives at its new home or was dropped on purpose. A shorter text counts as better only when nothing was lost.
- Describe the code from HEAD's vantage: no milestone codes, "this PR", "used to", or review history (see [references/leakage.md](references/leakage.md)).
- Cut slop:
  - filler openers and closers ("It is worth noting", "In summary");
  - implementation-status annotations on current-state docs ("(implemented)", "(done in M2)", "now supported", ✅ checklists); state the behaviour, and track progress only in plans;
  - walls of text: a paragraph over ~6 lines or a list item carrying several rules gets split, or turned into a list or table;
  - emphasis overuse: bold or caps mark the one word a reader must not miss or a list item's lead term, never whole sentences or paragraphs.
