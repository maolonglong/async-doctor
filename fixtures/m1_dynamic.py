import time


async def handler() -> None:
    getattr(time, "sleep")(1)
