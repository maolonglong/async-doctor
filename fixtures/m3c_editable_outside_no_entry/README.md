# m3c_editable_outside_no_entry

Clean-room for outside-scan-root editables as **follow-only**.

Layout:

- `app/` — scan root; `main.py` calls only `helper()`
- `other/libx/` — editable install target (`MAPPING` → `../other/libx/libx`)
  - `libx/jobs.py` exposes `helper()` (called) and never-called
    `async def unrelated_worker()` that does `time.sleep(5)`

Expect: **0 findings**. Calls into the editable are followable, but
`unrelated_worker` must not become a first-party entry (pre-fix: scanning
walked outside roots and reported the sleep as a finding).
