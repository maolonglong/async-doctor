"""Genexpr memo must clear per top-level consume across if/else branches."""

from __future__ import annotations

import asyncio
import time


async def consume_else_branch(flag: bool, items: list[int]):
    g = (time.sleep(1) for _ in items)
    if flag:
        await asyncio.to_thread(lambda: list(g))
    else:
        # Must still expand after the offloaded consume above.
        list(g)
