# Decisions

A decision record keeps the *why* and *what we gave up* that code and tests cannot carry, so nobody re-litigates a settled choice or silently reverses it.

Write one when a change picks between real alternatives that someone could plausibly re-propose: a dependency, a data format, a boundary, a rejected feature, a policy. Mechanical or local edits need none.

- Name: `<yyyy-mm-dd>-<slug>.md`, dated when first proposed. Browse this folder or search; there is no central index to keep in sync.
- `Status:` is one of `proposed`, `accepted`, `rejected: <one-line reason>`, `superseded by [<record>](<file>.md)`. `scripts/check_docs.py` rejects any other value and any `superseded by` link that does not resolve.
- Before writing a new record, search this folder for records on the same mechanism (`rg -il '<mechanism>' docs/decisions`). Mark each one the new record replaces as `superseded by <link>` in the same PR.
- A decision records the choice, not the work. Steps and acceptance criteria belong in an exec plan; an accepted record with `## Plan` or `## Acceptance criteria` fails the check.
- An accepted record states current reality in the present tense. Keep its facts (paths, names, defaults) current in the same PR that changes them.
- A reversed decision gets a new record that links the old one; the old one becomes `superseded by`. Never rewrite a record into a different decision.
- `Alternatives considered` is mandatory. Record alternatives that were actually weighed; do not invent them.
- A rejected idea is worth recording while it is still tempting.

Start from [_template.md](_template.md).
