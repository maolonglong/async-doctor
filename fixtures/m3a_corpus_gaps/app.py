"""M3a corpus-gap fixtures: redis await/attr, begin_entered, listen_*, mutation."""

from __future__ import annotations

import asyncio

import redis
from sqlalchemy.engine import Engine


class Holder:
    def __init__(self) -> None:
        self.client = redis.Redis()


async def redis_sync_await():
    # Sync get runs on the loop before await tries to await the result.
    r = redis.Redis()
    await r.get("k")


async def redis_sync_gather():
    r = redis.Redis()
    await asyncio.gather(r.get("a"), r.get("b"))


async def redis_watch():
    r = redis.Redis()
    r.watch("k")


async def redis_attr_client(h: Holder):
    h.client.get("k")


async def redis_union_optional(r: redis.Redis | None):
    # Without narrowing this may miss; positive once isinstance/None checks land.
    if r is not None:
        r.get("k")


async def redis_asyncio_await_ok():
    # Negative: awaited asyncio redis must not fire.
    r = redis.asyncio.Redis()
    await r.get("k")


async def begin_entered(engine: Engine):
    # Engine.begin blocks when entered via `with`.
    with engine.begin() as conn:
        conn.execute("select 1")


async def begin_not_entered_ok(engine: Engine):
    # Negative: constructing the context manager alone is not IO.
    _cm = engine.begin()


async def listen_consumed():
    r = redis.Redis()
    p = r.pubsub()
    p.subscribe("ch")
    for msg in p.listen():
        _ = msg


async def listen_comprehension():
    r = redis.Redis()
    p = r.pubsub()
    p.subscribe("ch")
    _ = [msg for msg in p.listen()]


async def listen_bare_ok():
    # Negative: creating the listen generator without consuming is not IO yet.
    r = redis.Redis()
    p = r.pubsub()
    _gen = p.listen()


async def unsafe_async_mutation(cache: dict, key: str, value: str):
    # Out of scope: shared-state mutation without blocking IO.
    # Documented in docs/plans/completed/2026-10-m3a-precision.md — async-doctor reports blocking IO,
    # not general concurrency / race hazards.
    cache[key] = value
