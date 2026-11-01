# Exec plans

An exec plan is a living design-and-progress document for work that outlives one session: several PRs, several agents, or hours of autonomous work. Anyone (a fresh agent, a human) must be able to resume from the plan file and the working tree alone.

- `active/`: plans in flight. Name: `<yyyy-mm-dd>-<slug>.md`, dated when started.
- `completed/`: finished plans, kept as history. Edit only to fix links.
- Small, obvious changes need no plan; the PR description is enough.

## Rules

- **Self-contained.** Define every term, name files by repo-relative path, and restate any context the work relies on. Link a decision record rather than re-arguing it.
- **Outcome-first.** Purpose says what a user can do afterwards; Validation says which command shows it and what output to expect.
- **Living.** Update Progress, Surprises & Discoveries, and Decision log at every stopping point, with UTC timestamps. Split a half-done item into done and remaining.
- **Milestones are independently verifiable.** Each one ends with a command whose result proves it.
- **Finish cleanly.** Write Outcomes & Retrospective, move the file to `completed/`, and copy leftover debt into [../quality.md](../quality.md).

Start from [active/_template.md](active/_template.md). `scripts/check_docs.py` requires its `##` sections in every plan.
