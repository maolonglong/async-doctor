"""Structural async-callee negatives: subclass, unnamed wrapper, AsyncOpenAI stream."""

from __future__ import annotations

import httpx
from openai import AsyncOpenAI


class MyClient(httpx.AsyncClient):
    async def request(self, method, url, **kw):
        return await super().request(method, url, **kw)


async def neg_subclass():
    c = MyClient()
    await c.get("http://x")


class Http:
    """Wrapper whose type name does not contain Async."""

    def __init__(self):
        self._c = httpx.AsyncClient()

    async def get(self, url: str):
        return await self._c.get(url)


async def neg_non_async_named_wrapper():
    await Http().get("http://x")


async def neg_async_openai_stream():
    async with AsyncOpenAI().chat.completions.stream(model="m", messages=[]) as s:
        async for _e in s:
            pass
