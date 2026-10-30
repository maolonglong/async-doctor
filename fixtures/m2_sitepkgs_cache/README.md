# m2_sitepkgs_cache

Stub `libx` under `.venv/.../site-packages` with a non-sink `fetch` that
calls `helper` → `time.sleep`. Exercises site-packages follow, package cache
keys (`libx.core.fetch`), and cold/warm parity.
