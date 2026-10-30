import asyncio
import time


def work() -> None:
    time.sleep(1)


async def offloaded() -> None:
    await asyncio.to_thread(work)


async def eager_bug() -> None:
    # Eagerly evaluates work() on the event loop before offloading the result.
    await asyncio.to_thread(work())
