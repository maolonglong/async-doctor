# Reject incremental per-file cache; keep whole-scan cache

Status: rejected: slower than main on measured hubs; complexity not worth the rare leaf wins

## Problem

Full re-extract on every first-party edit makes iterative agent loops pay cold
costs after small changes. A per-file incremental cache was prototyped to reuse
pass-1 facts across edits while recomputing follow/propagation.

## Decision

Do **not** merge incremental per-file caching. Keep the whole-scan report cache
(full key in [`../../ARCHITECTURE.md`](../../ARCHITECTURE.md)) and the existing
per-package third-party edge cache. The incremental effort lived on
https://github.com/maolonglong/async-doctor/pull/28 (closed unmerged; branch
kept for history).

Measured facts from that work: it matched full scans on 73 scenarios and a
220-edit random differential test on the seeds tried in that PR write-up, but
at close, random-edit seeds **23** and **1234** still found a correctness bug
(incremental output did not fully match cold). It was also slower than `main`
on every litellm edit (about 23s versus 13–14s; cold about +38%), saved only
about 9s on a dify leaf edit (17s versus 26s), showed dify cold about +22% and
hub edits about +42%, and added roughly 5.7k lines plus 22–33MB of cache per
project.

## Alternatives considered

- **Merge the v17 pass-1-only incremental file cache (PR #28).** Rejected on
  wall-clock and complexity: soundness work closed earlier gaps, but hub/edit
  paths still paid full-extract costs while growing cache size and code.
- **Whole-scan cache only (status quo on main).** Accepted: warm unchanged
  inputs skip ty extract/analyze; cold path stays one clear pipeline.

## Consequences

Agents must not revive per-file incremental caches without a new decision and
benchmarks that beat whole-scan warm + acceptable cold/edit costs. Related
benchmark notes: [../benchmark.md](../benchmark.md).
