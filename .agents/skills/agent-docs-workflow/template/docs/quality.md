# Quality and tech debt

Grades show where the codebase is solid and where agents should tread carefully. The debt tracker lists known shortcuts so they get paid down in small steps instead of piling up. The PR that adds or pays off debt updates them; a docs audit catches what drifted.

<!-- guide: Areas are the code-map modules from ARCHITECTURE.md plus cross-cutting concerns. Grades: A (well tested, clear boundaries), B (works, some gaps), C (fragile or unclear; read carefully before changing). Each grade carries a one-line reason. Delete guide comments once filled. -->

## Grades

| Area | Grade | Why | Updated |
|---|---|---|---|
| `<module>` | B | <one-line reason> | <yyyy-mm-dd> |

## Debt tracker

<!-- guide: One row per known shortcut. Remove the row in the PR that pays it off. Link the plan, issue, or note when one exists. -->

| Debt | Where | Cost of leaving it | Added |
|---|---|---|---|
| <what> | `<path>` | <what it slows down or risks> | <yyyy-mm-dd> |
