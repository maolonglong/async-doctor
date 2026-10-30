"""Positive fixture: direct time.sleep inside async def should be reported."""

import time


async def handler():
    time.sleep(1)
