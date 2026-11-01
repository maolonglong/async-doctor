# Architecture

<!-- guide: The map a new contributor (human or agent) needs to find where to change things. Write only what is unlikely to change often; revisit when a module, boundary, or invariant changes, and otherwise a few times a year. Name files, modules, and types in backticks so readers can symbol-search them; avoid links into source, which go stale (scripts/check_docs.py verifies that backticked repo paths still exist). Target under 400 lines. Delete guide comments once filled. -->

## Bird's-eye view

<!-- guide: 1-3 short paragraphs. What goes in, what comes out, what state is kept, what the system deliberately never does (e.g. "the core never does IO"). A reader should be able to predict the code map from this section. -->

<input> → <processing stages> → <output>.

## Entry points

<!-- guide: Where execution starts and where a newcomer should start reading. 2-5 bullets. -->

- `<path/to/main>`: <what starts here>.

## Code map

<!-- guide: One subsection per coarse module or directory, answering "where is the thing that does X?" and "what does the thing I am looking at do?". A map of the country, not an atlas of its states: details belong in module docs. Call out invariants (often an absence: "X never depends on Y") and API boundaries explicitly. If neighbours in this map are far apart in `tree`, consider moving code. -->

### `<path/module>`

<What it owns, in 1-3 sentences. Key types: `<Type>`, `<Type>`.>

**Architecture invariant:** <something that is true everywhere, often a thing deliberately absent>.

**API boundary:** <if other modules or users depend on this surface; what may change freely behind it>.

## Cross-cutting concerns

<!-- guide: Concerns that span modules. Keep the ones that apply, delete the rest. Each states the rule and where it is enforced (test, lint, CI step). -->

### Testing

<Where tests live, which boundary they target, how to update snapshots, what is slow and opt-in.>

### Error handling

<Which layers may fail, which must not, how failures are reported.>

### Performance

<Budgets, where benchmarks live, what not to regress.>

### Observability

<Logs, traces, profiling switches an agent can turn on.>
