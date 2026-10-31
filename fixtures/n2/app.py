import asyncio


async def _work():
    return 1


def create_idle_loop():
    # name says idle, but no loop-state check at all
    return asyncio.get_event_loop()


def n5(is_async: bool):
    # `is_async` here is a caller-supplied option, not a loop-state check
    if not is_async:
        return asyncio.run(_work())
    return None


def n6():
    loop = create_idle_loop()
    if loop is not None:
        loop.run_until_complete(_work())


def n7(main_loop=None):
    if main_loop is None:
        return asyncio.run(_work())


async def e5():
    return n5(False)


async def e6():
    return n6()


async def e7():
    return n7()
