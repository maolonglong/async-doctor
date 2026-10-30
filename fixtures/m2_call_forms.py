"""Lambda assign+call, blocking @property, genexpr consumed by next/list."""
import time


class Holder:
    @property
    def data(self):
        time.sleep(1)
        return 1


def gen():
    time.sleep(1)
    yield 1


async def lambda_assign():
    f = lambda: time.sleep(1)
    f()


async def property_access():
    h = Holder()
    _ = h.data


async def genexpr_next():
    next(x for x in gen())


async def genexpr_list():
    list(x for x in gen())


async def genexpr_deferred_then_list():
    g = (x for x in gen())
    list(g)
