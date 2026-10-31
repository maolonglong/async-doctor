import asyncio
from elasticsearch import Elasticsearch

def search():
    return Elasticsearch().search(index="i")

async def handler():
    await asyncio.to_thread(search)
