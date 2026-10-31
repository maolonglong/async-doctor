# Per-file incremental fact cache (design)

Schema **v15**. Complements the whole-scan report cache (exe-hash `build_id()`):
when some first-party files change, reuse extracted facts for the rest without
a full ty pass-2. Round-trips `advice[]` edge facts (`positional_externals`,
constructed receivers, `no_running_loop_guard`) so incremental reports match
cold byte-for-byte. Import heads resolve under the same search roots ty uses
(`src/`, config extras); wide misses skip slot persist so hub edits cost no
more than main cold.

## Goals

- Cold ≤ main (~15s litellm / 8 cores).
- Leaf and wide edits well under cold (wide edits may fall back to full extract).
- Incremental report **byte-identical** to a fresh cold on the edited tree.
- Sound under rename, delete, re-export delete, package-dir roots, offset
  shifts, same-second edits, new modules, multi-line imports, class bases,
  `__init__` re-exports, function moves, cycles, and `.pyi` add/edit/delete.

## Non-goals (this design)

- Caching third-party follow (already covered by the package cache).
- Skipping whole-scan when *nothing* changed (already covered by the scan cache).
- Keeping importers as hits across **body-only** callee edits (see below).

## Layout

```
{cache_root}/v15/{tool}/files/{project_hash}/{rel_hash}.json.gz
{cache_root}/v15/{tool}/files/{project_hash}/_project_meta.json
```

Slots are scoped by project root hash so fixture trees cannot collide.

## Hit condition

A first-party file `F` is a **hit** when all of:

1. `schema` / `tool` match.
2. `content_fingerprint(F)` matches (FNV of file bytes).
3. `venv_fingerprint` matches.
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
- synthetic **absent probes** (`rel.py` / `rel/__init__.py` with `"?"`) for
  unresolved / missing import heads (`add` / `addtop` / `newpkg`).

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
- Creating a previously missing module changes absent `?` probes into resolved
  content hashes → importers miss.

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
fall back to a full cold extract (no cache hits). New slots are still written
at the end.

## Orchestration

1. If whole-scan fingerprint hits → return cached report (unchanged).
2. Else, if any per-file slots exist: parallel-fingerprint the catalog; build
   the direct-import graph once; classify hits via content/venv/stub +
   `deps_fp`; optionally wide-miss fallback.
3. Pass-1/pass-2 only on misses (parallel pass-2 default; shared DB clones,
   no per-file `fresh_empty`).
4. Merge hits + miss extract; follow third-party; stabilize.
5. Encode+save miss slots in parallel (`EncodeIndexes` built once) with
   `deps_fp` from the shared direct graph + content map.
6. Save whole-scan report under the new fingerprint.

## Edge remapping

Cached Project edges store `(file, name_start, module, qualname)`. On restore,
accept a DefKey hit **only if** live `module`+`qualname` match; else fall back
to `(module, qualname)`; else treat the file as a miss via
`cached_project_callees_resolvable` (never emit a synthetic
`project:…` Unresolved that would poison `hiddenUnresolvedCandidates`).

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
