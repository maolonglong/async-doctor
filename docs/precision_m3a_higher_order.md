# M3a precision — higher-order, narrowing, in-memory IO

| Gap | Status | Notes |
| --- | --- | --- |
| Higher-order kwargs / `partial` / bound methods | **Fixed** | `keyword_callables` on call edges; materialization sets `context_from_caller` so reverse BFS does not leak a binding across other call sites of the same helper. |
| Type narrowing (`isinstance` / `is not None`) | **Fixed** | Cheap per-branch map only — no `and`/`or` explosion. |
| `PIL.Image.open(BytesIO)` / in-memory buffers | **Fixed** | When the path/fp argument type is an in-memory buffer, skip the open edge (no follow into PIL path guards). Path string opens still report. |
