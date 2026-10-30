"""Eager offload: `to_thread(f())` / `run_in_executor(None, f())` run f on the loop."""

import asyncio
import time


def work() -> int:
    time.sleep(1)
    return 1


async def eager_to_thread():
    await asyncio.to_thread(work())


async def eager_run_in_executor():
    loop = asyncio.get_running_loop()
    await loop.run_in_executor(None, work())


async def passed_to_thread_ok():
    # Negative: callable passed, not called.
    await asyncio.to_thread(work)


async def passed_run_in_executor_ok():
    loop = asyncio.get_running_loop()
    await loop.run_in_executor(None, work)
