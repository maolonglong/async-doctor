# Docs index

<!-- guide: The table of contents for docs/. Every top-level docs/*.md file is linked here (scripts/check_docs.py fails on orphans). Folders list their own contents in their README, so this file links folders, not each file in them. Delete guide comments once filled. -->

Repository docs are the system of record: if a fact lives only in chat, a PR thread, or someone's memory, agents cannot see it. Move it here.

## Where each fact lives

One home per fact; everywhere else links to it.

| Fact | Home | Not here |
|---|---|---|
| Orders every session needs | [`AGENTS.md`](../AGENTS.md), one line each, linking the reason | Stories, procedures, anything a linked doc already says |
| Module map, invariants, boundaries | [`ARCHITECTURE.md`](../ARCHITECTURE.md) | Per-function detail (code comments), rationale (decisions) |
| Why we chose X over Y | [`decisions/`](decisions/README.md) | Plans and checklists once shipped |
| Multi-step work in flight, with its log | [`plans/active/`](plans/README.md) | Finished work (move to `plans/completed/`) |
| Lessons, gotchas, incident write-ups | [`notes/`](notes/README.md) | Decisions (those go to `decisions/`) |
| Quality grades and known debt | [`quality.md`](quality.md) | Bugs with a clear fix (open an issue or just fix it) |
| Third-party docs pinned for agents | [`references/`](references/README.md) | Our own behaviour |
| How docs move with a task | [`workflow.md`](workflow.md) | |
| Facts a tool can print (`--help`, manifests, generated schemas) | The tool or a generated file | Hand-written copies, which go stale |

## Topic docs

<!-- guide: Project reference docs that do not fit a folder above (rule catalogue, CLI reference, benchmark method, product spec). One line each: link + what question it answers. -->

- [workflow.md](workflow.md): how a task flows through plans, code, and docs, and which checks enforce it.
- [quality.md](quality.md): per-area quality grades and the tech-debt tracker.
