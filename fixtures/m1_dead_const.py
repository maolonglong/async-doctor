"""Regression: unreassigned module constants prune dead branches."""

import time

DEBUG = False
_EMPTY = ()


async def dead_debug():
    if DEBUG:
        time.sleep(1)
    if _EMPTY:
        time.sleep(2)


async def live_not_empty():
    if not _EMPTY:
        time.sleep(1)
