import asyncio
import subprocess
import httpx
import pymongo
import requests

http = httpx.Client()
mongo = pymongo.MongoClient()


async def n1_requests():
    return await asyncio.to_thread(requests.get, "u")


async def n2_mongo():
    return await asyncio.to_thread(mongo.db.c.find_one, {})


def _mixed(url: str) -> None:
    data = http.get(url).content
    with open("/tmp/x", "wb") as f:
        f.write(data)


async def n3_mixed():
    await asyncio.to_thread(_mixed, "u")


async def n4_subprocess():
    await asyncio.to_thread(subprocess.run, ["ls"])


def _shared(url: str) -> str:
    return http.get(url).text


def cli_main() -> None:
    print(_shared("u"))


async def n5_shared_with_sync_cli():
    return await asyncio.to_thread(_shared, "u")


def _maybe(url: str, flag: bool):
    if flag:
        return http.get(url)
    return None


async def n6_conditional_sink():
    return await asyncio.to_thread(_maybe, "u", True)
