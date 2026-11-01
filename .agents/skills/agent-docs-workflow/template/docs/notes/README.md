# Notes

Notes hold what an agent cannot find by looking at the code: gotchas, environment quirks, debugging lessons, and incident write-ups. They are the repo's long-term memory, so a lesson learned in one session reaches every later one.

Write a note when a problem was **subtle** (a careful engineer would re-derive it the hard way), **systemic** (it escaped because of a gap in tests, tools, or conventions), or **costly to rediscover**. Skip one-off typos and anything the code or a test already makes obvious.

- Name: `<yyyy-mm-dd>-<slug>.md`.
- Lead with a two-sentence summary: what happened and the durable lesson.
- End with **Guardrail**: the test, lint, check, or `AGENTS.md` line that now catches it. A note with no guardrail is a candidate for one; the gardening pass promotes repeated lessons into tooling and then shortens or deletes the note.
- A note describes the world as it is now. When the code moves on and the lesson no longer applies, delete the note.

Start from [_template.md](_template.md).
