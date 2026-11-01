---
name: async-doctor-review
description: >-
  Review and verify async-doctor analysis or performance changes against the
  audit corpus. Use when changing call-graph extraction, propagation, sink or
  async-twin rules, or caching, or before cutting a release.
---
# async-doctor review

Every change to extraction, propagation, rules, or caching must pass a corpus
audit before merge. False positives are correctness bugs, and silent recall
drops are regressions.

## Workflow

1. **Build release binaries.** Compare the candidate binary against the baseline
   (either pre-recorded baseline JSONs or a release binary built from `main`).

   ```sh
   cargo build -p async_doctor --release
   ```

2. **Run the corpus audit.** Each repository scan must use an isolated
   `XDG_CACHE_HOME` and a `2400` second timeout so caches never bleed across
   corpora.

   ```sh
   python3 scripts/audit_compare.py run \
     --jobs <audit-dir>/jobs.txt \
     --root <audit-dir> \
     --baseline '<audit-dir>/baseline-c503648/{L}.i.json' \
     --binary ./target/release/async-doctor \
     --out /tmp/async-doctor-audit-out
   ```

   Or when comparing an existing output directory against a baseline directory:

   ```sh
   python3 scripts/audit_compare.py diff \
     '<audit-dir>/baseline-c503648/{L}.i.json' \
     /tmp/async-doctor-audit-out \
     --label head
   ```

3. **Evaluate the diff.** `scripts/audit_compare.py` classifies differences by
   finding ID and location key (`(rule, entry.file, entry.line, [(step.file, step.line), ...])`):
   - `added` (`+`): new findings not present in baseline.
   - `removed` (`-`): baseline findings missing from the new run.
   - `renumbered` (`~`): ID changed, but rule, entry location, and call-chain
     step locations are identical.

4. **Acceptance criteria:**
   - **Added (`+`)**: every added finding (or rule family) must be explained and
     verified as a true positive (never a false positive on async counterparts,
     in-memory buffers, off-loop thread guards, or `.done()`-guarded futures).
   - **Removed (`-`)**: must be **0**, or every removed finding must have an
     explicit, documented justification (e.g., proven false positive fixed by the
     change, or upstream repo commit drift when unpinned repos moved).
   - **Renumbered (`~`)**: must be explained by an intentional canonical sink or
     qualname normalization change.
   - **Cold vs warm parity**: on benchmark corpora (`./bench/run.sh`), cold and
     warm JSON outputs must be byte-identical (`identical=yes`).
