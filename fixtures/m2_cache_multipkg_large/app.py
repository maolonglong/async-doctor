from liba import fetch, cleanup, other
import time

async def handler_a():
    fetch("https://a.example")

async def handler_b():
    fetch("https://b.example")

async def handler_c():
    other("https://c.example")

async def handler_cleanup_unknown(path):
    cleanup(path)

async def handler_direct_sleep():
    time.sleep(1)

async def handler_d():
    fetch("https://d.example")
