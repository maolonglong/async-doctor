# m2_follow_budget_truncation

`liba.a0` → `a1` → `a2` → `libb.gate` → `deep_sleep` → `deep_lock` (`time.sleep` +
`threading.Lock.acquire`).

With injectable follow budget `2`:

- cold: `a0` free, then `a1`/`a2` consume the budget → never enters `libb`
- buggy warm (transitive cache materialization): `a0..a2` loaded free from cache,
  fresh budget then enters `libb` → extra sleep/lock hits

Warm must charge the same per-function budget as cold (no free transitive closure).
