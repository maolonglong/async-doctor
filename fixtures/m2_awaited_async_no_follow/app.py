import psycopg
import redis.asyncio as aioredis
from libz import fetch

async def use_async_connect():
    # Must NOT FP via AsyncConnection._connect_gen → time.sleep
    await psycopg.AsyncConnection.connect("dbname=test")

async def use_asyncio_redis_eval():
    r = aioredis.Redis()
    # Must NOT FP via commands.core._eval → os.remove
    await r.eval("return 1", 0)

async def use_libz_sleep():
    # Unknown third-party async that reaches sync IO — still a finding.
    await fetch("https://example.com")
