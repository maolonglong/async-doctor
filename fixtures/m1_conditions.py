"""Regression: prove-only conditions, param guards, cache-miss tags."""

import time

_cache = None


def maybe(flag):
    if flag:
        time.sleep(1)


async def branched(flag):
    if flag:
        time.sleep(1)
    else:
        time.sleep(2)


async def param_unknown(debug):
    maybe(debug)


async def param_false():
    maybe(False)


async def cache_miss():
    global _cache
    if _cache is None:
        time.sleep(1)
        _cache = 1
