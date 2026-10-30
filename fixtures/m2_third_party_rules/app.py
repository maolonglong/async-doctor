"""Positive: sync third-party clients; negative: async counterparts.

Stub packages live under `.venv/.../site-packages` so ty resolves External
qualnames and the built-in third-party rule table can fire.
"""

import anthropic
import boto3
import elasticsearch
import grpc
import httpx
import kafka
import openai
import psycopg2
import pymongo
import pymysql
import redis
import requests
import sqlalchemy
from sqlalchemy.orm import Session


async def use_requests():
    requests.get("https://example.com")


async def use_httpx_sync():
    client = httpx.Client()
    client.get("https://example.com")


async def use_httpx_async_ok():
    # Negative: AsyncClient must never fire.
    client = httpx.AsyncClient()
    await client.get("https://example.com")


async def use_redis_sync():
    r = redis.Redis()
    r.get("k")


async def use_redis_pipeline_buffering_ok():
    # Negative: set/get on a Pipeline only buffer — not findings.
    r = redis.Redis()
    pipe = r.pipeline()
    pipe.set("k", "v")
    pipe.get("k")


async def use_redis_pipeline_execute():
    r = redis.Redis()
    pipe = r.pipeline()
    pipe.set("k", "v")
    pipe.execute()


async def use_redis_async_ok():
    r = redis.asyncio.Redis()
    await r.get("k")


async def use_sqlalchemy_session(session: Session):
    session.execute("select 1")


async def use_sqlalchemy_async_ok():
    # Negative: AsyncSession must never fire.
    from sqlalchemy.ext.asyncio import AsyncSession

    session: AsyncSession
    await session.execute("select 1")  # type: ignore[misc]


async def use_psycopg2():
    psycopg2.connect("")


async def use_pymysql():
    pymysql.connect(host="localhost")


async def use_pymongo():
    client = pymongo.MongoClient()
    client.db.col.find({})


async def use_boto3():
    boto3.client("s3")


async def use_openai_sync():
    openai.OpenAI()


async def use_openai_async_ok():
    openai.AsyncOpenAI()


async def use_anthropic_sync():
    anthropic.Anthropic()


async def use_kafka():
    kafka.KafkaProducer().send("t", b"x")


async def use_elasticsearch():
    elasticsearch.Elasticsearch().search(index="i")


async def use_grpc_channel():
    # Sync channel construction — matched via type-based grpc rules when resolved.
    grpc.insecure_channel("localhost:50051")


async def use_redis_asyncio_pipeline_execute_ok():
    # Negative: awaited asyncio pipeline execute is not sync-IO.
    r = redis.asyncio.Redis()
    pipe = r.pipeline()
    await pipe.execute()
