"""Advice / special-rule fixtures: positives and negatives."""

from __future__ import annotations

import asyncio
from functools import partial

import httpx
import pymongo
import redis
import requests
from asgiref.sync import sync_to_async


def fetch_httpx():
    client = httpx.Client()
    return client.get("https://example.com")


def fetch_httpx_shared():
    client = httpx.Client()
    return client.get("https://example.com")


def fetch_redis():
    r = redis.Redis()
    return r.get("k")


def fetch_pymongo():
    return pymongo.MongoClient().db.col.find({})


def fetch_requests():
    return requests.get("https://example.com")


def fetch_mixed():
    client = httpx.Client()
    client.get("https://example.com")
    open("/tmp/x").read()


def sync_only_entry_calls_shared():
    """Sync entry never reached from async — exempts twin advice on shared fetch."""
    fetch_httpx_shared()


async def advice_to_thread_httpx():
    await asyncio.to_thread(fetch_httpx)


async def advice_run_in_executor_redis():
    loop = asyncio.get_running_loop()
    await loop.run_in_executor(None, fetch_redis)


async def advice_partial_httpx():
    await asyncio.to_thread(partial(fetch_httpx))


async def advice_lambda_httpx():
    def _inner():
        client = httpx.Client()
        return client.get("https://example.com")

    await asyncio.to_thread(_inner)


class Worker:
    def fetch(self):
        client = httpx.Client()
        return client.get("https://example.com")


async def advice_bound_method():
    w = Worker()
    await asyncio.to_thread(w.fetch)


async def advice_asgiref_sync_to_async():
    # Call-form offload boundary (decorator form is out of scope for v0 advice).
    wrapped = sync_to_async(fetch_httpx)
    await wrapped()


async def skip_pymongo_offload():
    await asyncio.to_thread(fetch_pymongo)


async def skip_requests_offload():
    await asyncio.to_thread(fetch_requests)


async def skip_mixed_sinks():
    await asyncio.to_thread(fetch_mixed)


async def skip_sync_caller_exemption():
    # fetch_httpx_shared is also called from sync_only_entry_calls_shared.
    await asyncio.to_thread(fetch_httpx_shared)


async def nested_loop_error():
    asyncio.run(asyncio.sleep(0))


def sync_nested_via_offload():
    asyncio.run(asyncio.sleep(0))


async def nested_behind_offload_ok():
    await asyncio.to_thread(sync_nested_via_offload)


async def offload_async_def_error():
    async def coro():
        return 1

    await asyncio.to_thread(coro)
