from dotenv import load_dotenv
from agents import Agent
from libx import fetch
import time

async def handler_fetch():
    fetch("https://example.com")

async def handler_dotenv():
    # Path-guarded open inside dotenv — candidate at most, must be stable cold/warm.
    load_dotenv(".env")

async def handler_agent():
    Agent().run("hi")

async def handler_sleep():
    time.sleep(1)
