from __future__ import annotations

import redis.asyncio as aioredis
from redis.asyncio import Pipeline, ClusterPipeline, Redis, RedisCluster

async def union_pipeline_set_ok(pipe: Pipeline | ClusterPipeline):
    # Must NOT fire blocking-db via CoreCommands.set → redis.Redis.set
    await pipe.set("k", "v")

async def union_client_pipeline_async_with_ok(r: Redis | RedisCluster):
    async with r.pipeline() as pipe:
        await pipe.set("k", "v")
        await pipe.execute()
