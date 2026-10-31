import time


def helper():
    return "ok"


async def unrelated_worker():
    time.sleep(5)
