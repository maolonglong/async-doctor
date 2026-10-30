"""Custom offload wrappers vs fake name-only wrappers."""
import asyncio
import time


def work():
    time.sleep(1)


async def real_wrapper(fn, *args):
    # Body actually offloads — inferred wrapper.
    return await asyncio.to_thread(fn, *args)


async def to_thread(fn, *args):
    # Same name as asyncio.to_thread but does NOT offload.
    return fn(*args)


async def safe():
    await real_wrapper(work)


async def unsafe():
    await to_thread(work)
