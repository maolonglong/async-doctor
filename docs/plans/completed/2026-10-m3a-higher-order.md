# M3a precision — higher-order, narrowing, in-memory IO

## Purpose

Close remaining M3a precision gaps around higher-order callables, cheap type
narrowing, and in-memory buffer opens that must not look like blocking file IO.

## Progress

- [x] 2026-10: Higher-order kwargs / `partial` / bound methods — fixed.
- [x] 2026-10: `isinstance` / `is not None` narrowing — fixed.
- [x] 2026-10: `PIL.Image.open(BytesIO)` / in-memory buffers — fixed.

## Surprises & Discoveries

| Gap | Status | Notes |
| --- | --- | --- |
| Higher-order kwargs / `partial` / bound methods | **Fixed** | `keyword_callables` on call edges; materialization sets `context_from_caller` so reverse BFS does not leak a binding across other call sites of the same helper. |
| Type narrowing (`isinstance` / `is not None`) | **Fixed** | Cheap per-branch map only — no `and`/`or` explosion. |
| `PIL.Image.open(BytesIO)` / in-memory buffers | **Fixed** | When the path/fp argument type is an in-memory buffer, skip the open edge (no follow into PIL path guards). Path string opens still report. |

## Decision log

- Decision: cheap per-branch narrowing only (no boolean explosion) / Date: 2026-10 / Rationale: keep extract bounded.
- Decision: skip open/follow when argument type is an in-memory buffer / Date: 2026-10 / Rationale: not blocking filesystem IO.

## Validation

- Higher-order / narrowing / buffer fixtures covered by `async_doctor_ty` m3a tests on main.

## Outcomes & Retrospective

Gaps closed. Bindings must not leak across call sites of the same helper —
regression coverage is the guardrail.
