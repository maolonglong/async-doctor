import asyncio
import httpx


async def offload(operation):
    return await asyncio.to_thread(operation)


async def after_guard(allowed):
    if not allowed:
        raise ValueError("denied")
    def read():
        return httpx.get("https://example.com")
    return await asyncio.to_thread(read)


async def inside_try():
    try:
        def read():
            return httpx.get("https://example.com")
        return await asyncio.to_thread(read)
    except ValueError:
        return None


async def inside_branch(allowed):
    if allowed:
        def read():
            return httpx.get("https://example.com")
        return await offload(read)


async def inside_loop(urls):
    for url in urls:
        def read():
            return httpx.get(url)
        await asyncio.to_thread(read)


async def internal_branch(allowed):
    def read():
        if allowed:
            return httpx.get("https://example.com")
    return await asyncio.to_thread(read)


async def mixed_sinks(allowed):
    if not allowed:
        return None
    def read():
        text = open("/tmp/example").read()
        return httpx.get(text)
    return await asyncio.to_thread(read)


def shared():
    return httpx.get("https://example.com")


def sync_caller():
    return shared()


async def shared_with_sync():
    return await asyncio.to_thread(shared)


async def dead_definition():
    if False:
        def read():
            return httpx.get("https://example.com")
        read()
