# m2_asyncio_redis_ctx

asyncio redis via `async with r.pipeline() as pipe` and `async for` over
`scan_iter` must never produce sync-IO findings. Sync `redis.Redis.get` still
fires.
