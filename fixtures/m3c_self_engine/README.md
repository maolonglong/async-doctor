# m3c_self_engine

- `SyncWorker`: `self.engine = create_engine(...)` then
  `with self.engine.begin()` / `with self.engine.connect()` in async methods
  → **blocking-db** findings.
- `AsyncWorker`: `self.engine = create_async_engine(...)` with
  `async with self.engine.begin()/connect()` → must stay clean.
