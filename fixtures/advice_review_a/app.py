import asyncio
import functools
import anthropic
import httpx
import openai
import redis
from openai import OpenAI

client = OpenAI()
aclient = anthropic.Anthropic()
http = httpx.Client()
r = redis.Redis()


async def p1_bound_method_unconditional():
    return await asyncio.to_thread(client.chat.completions.create, model="m", messages=[])


async def p2_in_if(flag: bool):
    if flag:
        return await asyncio.to_thread(client.chat.completions.create, model="m", messages=[])
    return None


async def p3_in_retry_loop():
    for _ in range(3):
        try:
            return await asyncio.to_thread(aclient.messages.create, model="m", max_tokens=1, messages=[])
        except Exception:
            pass


async def p4_lambda():
    return await asyncio.to_thread(lambda: client.chat.completions.create(model="m", messages=[]))


async def p5_partial():
    loop = asyncio.get_running_loop()
    return await loop.run_in_executor(None, functools.partial(http.get, "https://x"))


async def p6_redis_executor():
    loop = asyncio.get_running_loop()
    return await loop.run_in_executor(None, r.get, "k")


def _fetch(url: str) -> str:
    return http.get(url).text


async def p7_helper():
    return await asyncio.to_thread(_fetch, "https://x")


async def p8_module_client():
    return await asyncio.to_thread(openai.chat.completions.create, model="m", messages=[])


async def p9_httpx_module_get():
    return await asyncio.to_thread(httpx.get, "https://x")


async def p10_count_tokens():
    return await asyncio.to_thread(aclient.messages.count_tokens, model="m", messages=[])


def _fetch_with_ctx(url: str) -> str:
    with httpx.Client() as c:
        return c.get(url).text


async def p11_ctx_client_helper():
    return await asyncio.to_thread(_fetch_with_ctx, "https://x")


async def e1_nested():
    asyncio.run(asyncio.sleep(0))


async def _coro():
    return 1


async def e2_offload_coro():
    await asyncio.to_thread(_coro)


def _sync_runs_loop():
    return asyncio.run(_coro())


async def e3_nested_via_helper():
    return _sync_runs_loop()


def _guarded():
    try:
        asyncio.get_running_loop()
    except RuntimeError:
        return asyncio.run(_coro())
    return None


async def e4_guarded():
    return _guarded()


async def e5_offloaded_asyncio_run():
    return await asyncio.to_thread(asyncio.run, _coro())


async def e6_partial_coro():
    return await asyncio.to_thread(functools.partial(_coro))
