import asyncio
import httpx
from openai import AsyncOpenAI

ac = AsyncOpenAI()
ah = httpx.AsyncClient()


async def c1_async_ext_method():
    return await asyncio.to_thread(ac.chat.completions.create, model="m", messages=[])


async def c2_async_httpx():
    return await asyncio.to_thread(ah.get, "u")


async def c3_run_in_executor_async_ext():
    loop = asyncio.get_running_loop()
    return await loop.run_in_executor(None, ah.aclose)


def plain(x: int) -> int:
    return x


async def c4_ok_plain():
    return await asyncio.to_thread(plain, 1)
