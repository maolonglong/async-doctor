"""Regression: generator body only propagates when consumed."""

import time


def gen():
    time.sleep(1)
    yield 1


async def bare_call():
    g = gen()


async def for_consume():
    for _ in gen():
        pass


async def list_consume():
    list(gen())
