# AGENTS.md

<!-- guide: This file is a map, loaded into every agent session. It stays under 100 lines (scripts/check_docs.py fails above that). Each line is either a standing order most tasks need, or a pointer that names a deeper doc and the condition for reading it. Replace every <placeholder>, then delete all guide comments. -->

<One sentence: what this project is, who runs it, and the one outcome it must never break.>

## Where to look

<!-- guide: Pointers, not content. Lead each line with the trigger ("Before X", "When Y"), then the link. One line per distinct case. -->

- Before changing code in more than one module, read [ARCHITECTURE.md](ARCHITECTURE.md): code map, invariants, boundaries.
- For work that spans several PRs, sessions, or agents, start or resume an exec plan in [docs/plans/](docs/plans/README.md).
- Before reversing a design choice or re-proposing a rejected idea, search [docs/decisions/](docs/decisions/README.md).
- When a tool, dependency, or test behaves surprisingly, search [docs/notes/](docs/notes/README.md) first.
- To find where a fact belongs, or how docs move with a task, read [docs/README.md](docs/README.md) and [docs/workflow.md](docs/workflow.md).
- Repo skills live in `.agents/skills/<name>/SKILL.md`; each has a symlink at `.claude/skills/<name>` for Claude Code.

## Commands

<!-- guide: Only what one look at the manifest (Makefile, package.json, pyproject, Cargo.toml) would not tell an agent: the fast default test, flags that matter, slow suites that are opt-in. -->

```sh
<fast test command>                # default before every push
<lint / format check>
python3 scripts/check_docs.py      # doc links, size budgets, plan and decision structure
```

## Standing orders

<!-- guide: 5 to 15 one-line rules that change agent behaviour versus its default. Phrase each as the target behaviour. Link each to the decision or note that holds its reason. A rule a linter or test can check belongs in that check; keep only a pointer here. -->

- Keep files near 500 lines; 800 is the ceiling. Split by responsibility, keeping the module interface unchanged.
- Docs change in the same PR as the behaviour they describe.
- <project rule> ([why](docs/decisions/<file>.md))

## Done means

<!-- guide: The completion criterion every PR meets. Each item is observable. -->

- `<fast test command>` and `python3 scripts/check_docs.py` pass.
- The exec plan (if any) shows the real state in Progress and Decision log; a finished plan moved to `docs/plans/completed/`.
- ARCHITECTURE.md matches any module, boundary, or invariant this PR changed.
- Anything learned the hard way is a note, a test, or a lint, so the next agent does not rediscover it.
