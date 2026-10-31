import asyncio
import os
from openai import OpenAI

openai_client = OpenAI(api_key=os.getenv("OPENAI_API_KEY"))


async def s1(messages):
    response = await asyncio.to_thread(
        lambda: openai_client.chat.completions.create(model="gpt-4o-mini", messages=messages)
    )
    return response


async def s2(messages):
    try:
        response = await asyncio.to_thread(
            lambda: openai_client.chat.completions.create(model="gpt-4o-mini", messages=messages)
        )
    except Exception as e:
        if "insufficient_quota" in str(e):
            response = await asyncio.to_thread(
                lambda: openai_client.chat.completions.create(model="gpt-4o", messages=messages)
            )
        else:
            raise
    return response


async def s3(messages):
    try:
        return await asyncio.to_thread(
            lambda: openai_client.chat.completions.create(model="gpt-4o-mini", messages=messages)
        )
    except Exception:
        return None


async def s4(messages):
    r1 = await asyncio.to_thread(lambda: openai_client.chat.completions.create(model="a", messages=messages))
    r2 = await asyncio.to_thread(lambda: openai_client.chat.completions.create(model="b", messages=messages))
    return r1, r2
