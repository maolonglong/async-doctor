import redis
import redis.asyncio as aioredis

async def sync_redis_get_blocks():
    r = redis.Redis()
    r.get("k")

async def asyncio_pipeline_async_with_set_ok():
    r = aioredis.Redis()
    async with r.pipeline() as pipe:
        await pipe.set("k", "v")
        await pipe.execute()

async def asyncio_scan_iter_ok():
    client = aioredis.Redis()
    async for k in client.scan_iter(match="*"):
        await client.get(k)

async def asyncio_eval_await_ok():
    r = aioredis.Redis()
    await r.eval("return 1", 0)
