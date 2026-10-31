import asyncio
import httpx

def fetch():
    return httpx.Client().get("https://example.com")

async def handler():
    await asyncio.to_thread(fetch)
