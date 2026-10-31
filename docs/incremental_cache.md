# Per-file incremental fact cache (design)

Schema **v17**. Complements the whole-scan report cache (exe-hash `build_id()`):
when some first-party files change, reuse **pass-1 extract facts** for the rest
without a full ty pass-2. **No global / derived state is restored from cache** —
third-party follow, propagation, grouping, sink sites, counters, and advice are
always recomputed from the merged pass-1 facts. Round-trips `advice[]` edge
facts (`positional_externals`, constructed receivers, `no_running_loop_guard`)
so incremental reports match cold byte-for-byte.

## Contract (v17)

Cache **only** per-file pass-1 facts, a pure function of:

`(file bytes, deps_fp / closure hash, search-root + venv/editable fingerprint, exe/schema)`

On every run:

1. Classify hits/misses via content + env + `deps_fp`.
2. Merge hit slots + re-extract misses.
3. **Always** re-run third-party follow, stabilize, analyze/propagate, and
   advice from the merged facts.
4. Persist miss slots (including wide-miss full-extract fallback) so the cache
   recovers after hub edits.

Per-file slots store the `follow_targets` discovered while extracting that
file's External edges (still a pass-1 fact keyed by the file + venv
fingerprint). Follow itself is never restored as a global graph.

## Goals

- Cold ≤ main (~15s litellm / 8 cores).
- Leaf and wide edits well under cold when the hit path applies; wide edits
  may fall back to full extract but still persist slots.
- Incremental report **byte-identical** to a fresh cold on the edited tree.
- Sound under rename, delete, re-export delete, package-dir roots, offset
  shifts, same-second edits, new modules, multi-line imports, class bases,
  `__init__` re-exports, function moves, cycles, `.pyi` add/edit/delete,
  multi-root / `src/` shadowing, editable finders, and `.pth` path hooks.

## Non-goals (this design)

- Skipping whole-scan when *nothing* changed (already covered by the scan cache).
- Keeping importers as hits across **body-only** callee edits (see below).

## Layout

```
{cache_root}/v17/{tool}/files/{project_hash}/{rel_hash}.json.gz
{cache_root}/v17/{tool}/files/{project_hash}/_fp_index.json.gz
{cache_root}/v17/{tool}/files/{project_hash}/_project_meta.json
```

Slots are scoped by project root hash so fixture trees cannot collide.
`_fp_index.json.gz` stores only content/deps/venv/stub fingerprints so hit
classification (and wide-miss fallback) never gunzips full slots.

## Hit condition

A first-party file `F` is a **hit** when all of:

1. `schema` / `tool` match.
2. `content_fingerprint(F)` matches (FNV of file bytes).
3. `venv_fingerprint` matches (dist-info + editable finder bodies + `.pth`).
4. `stub_fingerprint` matches (or was empty at save — treated as wildcard once).
5. Live-recomputed `deps_fp(F)` equals the stored `deps_fp`.

Otherwise `F` is a **miss** and is re-extracted.

## Transitive AST-import `deps_fp` (sound for import-flow inference)

Cross-module inference in the review scenarios flows through imports
(factory→impl→bases, mid2→bases, re-exports, …). Each slot stores:

```
deps_fp(F) = FNV("deps-fp-v1\n" + sorted lines "{rel}\t{content_fp_or_?}\n")
```

over:

- every first-party file in the **transitive import closure** of `F`
  (AST-scraped `import` / `from … import`, including nested function/class
  bodies; exact catalog match + package-root aliases only — **no** parent
  fallback), and
- synthetic **absent probes** for unresolved / missing import heads, including
  **higher-precedence candidate paths** across every ty search root (`src/`,
  configured extras, editable finder mappings, `.pth` paths) **up to and
  including** the resolved winner — so a new higher-precedence file
  (e.g. root `pkg/m.py` over `src/pkg/m.py`) flips importers' `deps_fp`.

The direct-import graph is built **once** in parallel over the catalog
(rayon). Hit checks recompute `deps_fp` from the live graph + live content
hashes — no per-file salsa `Files` reset and no filesystem recording.

Invalidation falls out of live hashes:

- Edit `M` → content miss for `M`.
- Any importer `I` whose transitive closure includes `M` recorded the **old**
  hash of `M`; the live hash is **new** → `I` misses too.
- Delete `M` → live lookup fails / hash missing → importers miss.
- Body-only edit inside `M` still changes `M`'s content hash → importers that
  transitively import `M` miss. Intentional: cross-module inference can depend
  on bodies in ways an exported-signature hash cannot see.
- Creating a previously missing / higher-precedence module changes absent `?`
  probes into resolved content hashes → importers miss.
- Editable mapping / `.pth` content changes flip `venv_fingerprint` → all slots
  miss.

### Removed: consulted-file RecordingSystem / `consulted_fps`

An earlier draft recorded every first-party `.py`/`.pyi` path probed during
pass-2 via a `RecordingSystem` wrapper and a **fresh salsa `Files` table per
file**. That was sound but made cold litellm ~7× slower than main. It is
replaced by `deps_fp`. Slots that still have empty `deps_fp` (legacy) miss.

### Removed: `interface_fp` import keys

Still gone: keyed imports as `module → interface_fp(M)` was unsound for ty's
cross-module inference and O(files × facts) to recompute.

## Wide-edit fallback

If the miss count exceeds **50% of the catalog** or an absolute cap (**500**),
fall back to a full cold extract (no cache hits) and **still persist** miss
slots so subsequent leaf edits recover the warm path. Classification uses
`_fp_index` (or aborts early while counting) before pass-1/pass-2.

## Orchestration

1. If whole-scan fingerprint hits → return cached report (unchanged).
2. Else, if any per-file slots exist: parallel-fingerprint the catalog; classify
   via `_fp_index` (content/venv/stub + `deps_fp`); optionally wide-miss
   fallback before loading full slots.
3. Pass-1/pass-2 only on misses (parallel pass-2 default; shared DB clones,
   no per-file `fresh_empty`).
4. Merge hits + miss extract; **always** follow third-party; stabilize; analyze.
5. Encode+save miss slots in parallel (`EncodeIndexes` built once) with
   `deps_fp` from the shared direct graph + content map; upsert `_fp_index`.
6. Save whole-scan report under the new fingerprint.

## Edge remapping

Cached Project edges store `(file, name_start, module, qualname)`. On restore,
accept a DefKey hit **only if** live `module`+`qualname` match; else fall back
to `(module, qualname)`; else treat the file as a miss via
`cached_project_callees_resolvable` (never emit a synthetic
`project:…` Unresolved that would poison `hiddenUnresolvedCandidates`).
Edge `file` is omitted when equal to the slot `rel`.

## Differential harness

`tests/file_cache/differential.rs` (+ `scripts/differential_cache.sh`):

- Apply N seeded random edits (body / annotation / add / delete / rename /
  import / site-packages chain) to every `fixtures/` entry.
- After each edit, assert incremental JSON is **byte-identical** to a
  fresh-cache cold scan.
- CI: fixed seeds (`1,42`) and small N (`2`); override with
  `ASYNC_DOCTOR_DIFF_SEEDS` / `ASYNC_DOCTOR_DIFF_EDITS`.
- Local longer: `./scripts/differential_cache.sh --extended`.

## `.pyi`

Stub add/edit/delete already busts the **whole-scan** fingerprint. For the
per-file path, `stub_fingerprint` over first-party `.pyi` paths invalidates
all file slots when the stub set changes.

## Perf notes

- Pass-2 matches main: `template_db.clone()` per worker, salsa file cache from
  pass-1 reused.
- Direct-import graph: one parallel scrape over the catalog (AST parse only).
- Cold with no slots: skip fingerprinting until save; then fingerprint once
  for `deps_fp` encode.
- One-pass encode indexes; parallel save; Arc share + overlays in pass-2.
- Slots are gzip'd JSON with empty/default fields omitted; files-cache size is
  dominated by first-party extract facts (not a restored global follow graph).
