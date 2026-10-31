import asyncio
from openai import AzureOpenAI, OpenAI

az = AzureOpenAI(api_key="k", api_version="2024-02-01", azure_endpoint="https://x")
oa = OpenAI(api_key="k")


def get_client() -> AzureOpenAI:
    return AzureOpenAI(api_key="k", api_version="2024-02-01", azure_endpoint="https://x")


async def z1():
    return await asyncio.to_thread(az.chat.completions.create, model="m", messages=[])


async def z2():
    c = get_client()
    return await asyncio.to_thread(c.chat.completions.create, model="m", messages=[])


async def z3():
    return await asyncio.to_thread(lambda: az.chat.completions.create(model="m", messages=[]))


async def z4():
    return await asyncio.to_thread(oa.chat.completions.create, model="m", messages=[])
