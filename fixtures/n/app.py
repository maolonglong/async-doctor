import asyncio


async def _work():
    return 1


def loop_is_enabled() -> bool:
    return True


def get_loop_config():
    return None


def is_looping_feature_on() -> bool:
    return True


def n1(flag):
    # helper name contains 'loop' but checks a feature flag, not loop state
    if loop_is_enabled():
        return asyncio.run(_work())
    return None


def n2():
    cfg = None if get_loop_config() else object()
    if cfg is not None:
        return asyncio.run(_work())
    return None


def n3():
    x = None if is_looping_feature_on() else 1
    if x is None:
        asyncio.get_event_loop().run_until_complete(_work())


def n4():
    loop = asyncio.new_event_loop()
    if loop is not None:  # not a running-loop check
        loop.run_until_complete(_work())


async def e_n1():
    return n1(True)


async def e_n2():
    return n2()


async def e_n3():
    return n3()


async def e_n4():
    return n4()


# true guard via helper ternary (should be dropped)
def is_event_loop_running() -> bool:
    try:
        asyncio.get_running_loop()
        return True
    except RuntimeError:
        return False


def g1():
    idle = None if is_event_loop_running() else asyncio.new_event_loop()
    if idle is not None:
        idle.run_until_complete(_work())


async def e_g1():
    return g1()
