"""Clean-room thread-identity guards (Home Assistant frame.py shape, no HA code)."""

from __future__ import annotations

import threading
from concurrent.futures import Future


# Pretend event-loop thread id (set at import, like hass.loop_thread_id).
loop_thread_id = threading.get_ident()


def run_callback_threadsafe(loop_id: int, fn):
    """Stand-in for asyncio.run_coroutine_threadsafe / hass helper."""
    fut: Future = Future()
    fut.set_result(fn())
    return fut


async def off_loop_get_ident_result_ok():
    # Body only runs off the loop thread — .result() must not be a finding.
    if loop_thread_id != threading.get_ident():
        run_callback_threadsafe(loop_thread_id, lambda: 1).result()


async def off_loop_main_thread_result_ok():
    if threading.current_thread() is not threading.main_thread():
        fut: Future = Future()
        fut.set_result(0)
        fut.result()


async def on_loop_else_result_ok():
    if loop_thread_id == threading.get_ident():
        return None
    # Complementary else after on-loop early path is modeled via != above;
    # this branch is the != form again for coverage.
    if loop_thread_id != threading.get_ident():
        fut: Future = Future()
        fut.set_result(0)
        fut.result()


async def unguarded_result_blocks():
    fut: Future = Future()
    fut.result(timeout=1)


async def done_guard_result_ok():
    fut: Future = Future()
    fut.set_result(1)
    if fut.done():
        fut.result()
