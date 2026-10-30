# M3a corpus-gap fixtures

Covers the known M3a recall cases (and matching negatives):

| Case | Expected |
| --- | --- |
| `redis_sync_await` / `redis_sync_gather` / `redis_watch` / `redis_attr_client` | findings |
| `redis_asyncio_await_ok` | no finding |
| `begin_entered` | finding (`with Engine.begin()`) |
| `begin_not_entered_ok` | no finding |
| `listen_consumed` / `listen_comprehension` | findings |
| `listen_bare_ok` | no finding |
| `unsafe_async_mutation` | **out of scope** (no finding; see `docs/precision_m3a.md`) |

Stub `redis` + `sqlalchemy` live under `.venv/.../site-packages`.
