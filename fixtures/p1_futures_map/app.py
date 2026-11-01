"""Executor.map consumption and futures.wait must report; submit alone does not."""

from concurrent.futures import ThreadPoolExecutor, wait, Future
import asyncio


def work(x):
    return x


async def map_consumed():
    with ThreadPoolExecutor() as ex:
        list(ex.map(work, [1, 2]))


async def wait_blocked():
    fut = ThreadPoolExecutor().submit(work, 1)
    wait([fut])


async def submit_only_ok():
    # Returning a Future without .result() / wait is not a sink by itself.
    return ThreadPoolExecutor().submit(work, 1)


async def asyncio_future_result(loop):
    # Must not be confused with concurrent.futures.Future.result.
    f = loop.create_future()
    f.set_result(1)
    return f.result()
