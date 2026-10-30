# m2_follow_budget_truncation

`liba.a0` → `a1` → `a2` → `libb.gate` → `time.sleep`.

With injectable follow budget `2`:

- cold: `a0` free, then `a1`/`a2` consume the budget → never enters `libb`
- warm must replay the same cut (no free transitive cache materialization that
  would keep a full budget and continue into `libb`)

See `tests::budget::m2_follow_budget_cold_warm_json_identical_when_truncated`.
