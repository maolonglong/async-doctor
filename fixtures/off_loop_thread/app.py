"""Clean-room thread-identity guards (Home Assistant frame.py shape, no HA code)."""

from __future__ import annotations

import threading
from concurrent.futures import Future, ThreadPoolExecutor


# Pretend event-loop thread id (set at import, like hass.loop_thread_id).
loop_thread_id = threading.get_ident()
ex = ThreadPoolExecutor(max_workers=1)


def run_callback_threadsafe(loop_id: int, fn):
    """Stand-in for asyncio.run_coroutine_threadsafe / hass helper."""
    fut: Future = Future()
    fut.set_result(fn())
    return fut


def work():
    return 1


async def off_loop_get_ident_result_ok():
    # Body only runs off the loop thread — .result() must not be a finding.
    if loop_thread_id != threading.get_ident():
        run_callback_threadsafe(loop_thread_id, lambda: 1).result()


async def off_loop_attr_loop_thread_id_ok():
    class Hass:
        loop_thread_id = threading.get_ident()

    hass = Hass()
    if hass.loop_thread_id != threading.get_ident():
        fut: Future = Future()
        fut.set_result(0)
        fut.result()


async def off_loop_main_thread_result_ok():
    if threading.current_thread() is not threading.main_thread():
        fut: Future = Future()
        fut.set_result(0)
        fut.result()


async def on_loop_else_result_ok():
    if loop_thread_id == threading.get_ident():
        return None
    if loop_thread_id != threading.get_ident():
        fut: Future = Future()
        fut.set_result(0)
        fut.result()


async def lock_owner_get_ident_still_blocks():
    """Arbitrary owner != get_ident() is NOT an off-loop guard."""

    class Lock:
        owner = 0

    lk = Lock()
    if lk.owner != threading.get_ident():
        ex.submit(work).result()


async def unguarded_result_blocks():
    fut: Future = Future()
    fut.result(timeout=1)


async def done_guard_result_ok():
    fut: Future = Future()
    fut.set_result(1)
    if fut.done():
        fut.result()


async def done_guard_attr_and_ok():
    class Holder:
        def __init__(self):
            self.x: Future = Future()
            self.x.set_result(1)

    h = Holder()
    if h.x.done() and h.x.result() is not None:
        return


async def done_guard_ternary_ok():
    fut: Future = Future()
    fut.set_result(1)
    _ = fut.result() if fut.done() else None


async def done_guard_attr_if_ok():
    class Holder:
        def __init__(self):
            self.x: Future = Future()
            self.x.set_result(1)

    h = Holder()
    if h.x.done():
        return h.x.result()


async def done_guard_rebind_still_blocks():
    """Rebinding `fut` inside `if fut.done():` invalidates the guard."""
    fut: Future = Future()
    fut.set_result(1)
    if fut.done():
        fut = Future()
        fut.result()
