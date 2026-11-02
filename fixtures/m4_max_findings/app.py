import time


def _helper():
    time.sleep(1)


async def via_a():
    _helper()


async def via_b():
    _helper()


async def direct():
    time.sleep(2)
