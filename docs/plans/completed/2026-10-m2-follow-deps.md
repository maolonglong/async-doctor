# M2 — Follow dependencies

## Purpose

Follow third-party functions reachable from project async entries down to
stdlib sinks, add a versioned package cache and whole-scan cache foundation,
and ship type-based third-party rules with async-twin hints.

## Progress

- [x] Third-party follow with relevance-ranked budget; site-/dist-packages never entry sources.
- [x] Per-`(package, version, content-fingerprint)` disk cache under the user cache dir; per-scan `PackageMetaMemo`; injectable cache root for tests.
- [x] Whole-scan report cache keyed on content hashes + options + tool/schema (warm skips extract).
- [x] Built-in third-party rule table; redis Pipeline buffering not IO; higher-order kwargs/partial; ty-resolved offload + inferred wrappers.
- [x] Venv discovery + zero-config extra search paths (pyright/ty/ruff/setuptools editable).
- [x] Precision notes: [2026-10-m2-precision.md](2026-10-m2-precision.md).

## Surprises & Discoveries

- Attribute Externals prefer receiver type over mixin defining classes (redis `Pipeline` vs `CoreCommands`).
- Project subclasses of `redis.Redis` / `redis.asyncio.Redis` inherit sink status from receiver MRO.
- Only the Call that is the direct operand of `await` / `async for` / `async with` is `is_awaited`.
- Content fingerprint is coarse (RECORD/METADATA/one-level `.py`); nested non-`.py` / C-ext edits can miss.

## Decision log

- Decision: both venv site-packages and system dist-packages are third-party / Date: 2026-10 / Rationale: never treat library async defs as finding entries.
- Decision: callers inject cache root; do not mutate `XDG_CACHE_HOME` under parallel tests / Date: 2026-10 / Rationale: process-global races.

## Validation

- `fixtures/m2_*` + m2 precision determinism test; cold JSON == warm JSON on sitepkgs cache fixture.

## Outcomes & Retrospective

M2 shipped follow + caches + third-party rules. Later parallel pass-2 and
jemalloc improved cold/warm further ([../../benchmark.md](../../benchmark.md)).
Incremental per-file cache later rejected
([../../decisions/2026-11-01-reject-incremental-file-cache.md](../../decisions/2026-11-01-reject-incremental-file-cache.md)).
