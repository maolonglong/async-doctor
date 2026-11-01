# AGENTS.md

async-doctor is a zero-config Rust CLI that finds sync/blocking IO reachable
from Python `async def` code and reports evidence agents can act on; never
ship a false positive as a finding, and never add a project config file.

## Where to look

- Before changing code in more than one crate or module, read [ARCHITECTURE.md](ARCHITECTURE.md): code map, invariants, boundaries.
- For work that spans several PRs, sessions, or agents, start or resume an exec plan in [docs/plans/](docs/plans/README.md).
- Before reversing a design choice or re-proposing a rejected idea, search [docs/decisions/](docs/decisions/README.md).
- When a tool, dependency, or test behaves surprisingly, search [docs/notes/](docs/notes/README.md) first.
- To find where a fact belongs, or how docs move with a task, read [docs/README.md](docs/README.md) and [docs/workflow.md](docs/workflow.md).
- When editing rules, JSON shape, or CLI flags users see, update the matching topic doc ([docs/rules.md](docs/rules.md), [docs/json-contract.md](docs/json-contract.md), [docs/cli.md](docs/cli.md)).
- Repo skills live in `.agents/skills/<name>/SKILL.md`; each has a symlink at `.claude/skills/<name>` for Claude Code. The product skill stays at `skills/async-doctor/SKILL.md`.

## Commands

```sh
cargo test --workspace                              # default before every push
cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings
./scripts/check_rust_file_length.sh                 # crates/**/*.rs <= 800 lines
python3 scripts/check_docs.py                       # doc links, size budgets, plan/decision structure
cargo test -p async_doctor --test concurrent_cache -- --ignored   # cold-cache race stress (opt-in)
```

## Standing orders

- Follow `.agents/skills/fast-rust` for all Rust work; follow `.agents/skills/codebase-design` when adding or restructuring modules (deep modules, small interfaces, seams where something actually varies).
- Keep source files near 500 lines; 800 is the hard ceiling (`scripts/check_rust_file_length.sh`). Split by responsibility into a module directory, keeping the public interface unchanged; large test suites go under `tests/` per area, not one growing `tests.rs`. Fixtures and generated data are exempt.
- Stay zero-config: entries, rules, and offload boundaries are built-in or inferred; the only escape hatch is `# async-doctor: ignore[rule]: <reason>` ([why](docs/decisions/2026-10-30-zero-config.md)).
- Clean-room only: public docs and crates; do not copy from react-doctor or unlicensed sources; all ty/ruff use stays in `async_doctor_ty` ([why](docs/decisions/2026-10-30-clean-room-mit.md)).
- False positives are correctness bugs: uncertain cases go to `candidates` or `advice`, never `findings`; advice never changes the exit code ([why](docs/decisions/2026-10-31-advice-tier.md)).
- Prefer the react-doctor product shape: ship the CLI and `skills/async-doctor/SKILL.md` install path; no editor stop hooks ([why](docs/decisions/2026-10-31-no-stop-hook.md)).
- Keep the whole-scan cache (content + executable hash); do not revive per-file incremental caches without a new decision ([why](docs/decisions/2026-11-01-reject-incremental-file-cache.md)).
- Docs change in the same PR as the behaviour they describe.
- All repo docs are English only.

## Done means

- `cargo test --workspace` and `python3 scripts/check_docs.py` pass.
- The exec plan (if any) shows the real state in Progress and Decision log; a finished plan moved to `docs/plans/completed/`.
- `ARCHITECTURE.md` matches any module, boundary, or invariant this PR changed.
- Anything learned the hard way is a note, a test, or a lint, so the next agent does not rediscover it.
