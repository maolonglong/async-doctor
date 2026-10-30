"""Awaited async HTTP clients must never become blocking findings."""

from __future__ import annotations

import httpx
from elasticsearch import AsyncElasticsearch


async def async_httpx_get_ok():
    async with httpx.AsyncClient() as c:
        await c.get("http://x")


async def async_elasticsearch_search_ok():
    await AsyncElasticsearch().search(index="i")
