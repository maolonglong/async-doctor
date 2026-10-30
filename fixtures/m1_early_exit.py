"""Regression: early-exit guards condition subsequent sinks; always-true prunes."""

import time

_EMPTY = ()
_cache = None
_map = {}


async def after_return(x):
    if x:
        return
    time.sleep(1)


async def after_raise(ok):
    if not ok:
        raise ValueError("no")
    time.sleep(1)


async def after_cache_hit():
    global _cache
    if _cache is not None:
        return _cache
    time.sleep(1)
    _cache = 1


async def after_key_in_cache(key):
    if key in _map:
        return _map[key]
    time.sleep(1)


async def unreachable_after_empty():
    if not _EMPTY:
        return
    time.sleep(1)


async def comprehension(xs, flag):
    def block():
        time.sleep(1)
        return 1

    _ = [block() for x in xs if flag]
