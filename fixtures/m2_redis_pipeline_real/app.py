"""Real redis hierarchy cases for Pipeline receiver-type rules."""

import redis
import redis.asyncio as aioredis


async def sync_pipeline_set_ok():
    # Negative: Pipeline.set only buffers (mixin-defined, receiver is Pipeline).
    r = redis.Redis()
    pipe = r.pipeline()
    pipe.set("k", "v")
    pipe.get("k")


async def sync_pipeline_execute_blocks():
    r = redis.Redis()
    pipe = r.pipeline()
    pipe.set("k", "v")
    pipe.execute()


async def sync_redis_get_blocks():
    r = redis.Redis()
    r.get("k")


async def asyncio_pipeline_execute_ok():
    # Negative: await pipe.execute() must never be a sync-IO finding.
    r = aioredis.Redis()
    pipe = r.pipeline()
    await pipe.set("k", "v")
    await pipe.execute()


async def asyncio_pipeline_set_ok():
    r = aioredis.Redis()
    pipe = r.pipeline()
    await pipe.set("k", "v")
