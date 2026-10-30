# m2_awaited_async_no_follow

Awaited known-async APIs (`psycopg.AsyncConnection.connect`, `redis.asyncio`
`eval`) must not be followed into library internals. Unknown TP async
(`libz.fetch` → `time.sleep`) still propagates to the first-party entry.
