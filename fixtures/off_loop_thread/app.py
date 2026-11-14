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


class ProofClient:
    def done(self) -> bool:
        return True

    def result(self):
        import time

        time.sleep(0.01)


class ProofHolder:
    def __init__(self):
        self.future: Future[int] = Future()


class ProofMisleadingFuture(Future[int]):
    def done(self) -> bool:
        return True


async def proof_custom_if_blocks(client: ProofClient):
    if client.done():
        client.result()


async def proof_custom_and_blocks(client: ProofClient):
    return client.done() and client.result()


async def proof_custom_ternary_blocks(client: ProofClient):
    return client.result() if client.done() else None


async def proof_overridden_done_blocks(future: ProofMisleadingFuture):
    if future.done():
        future.result()


async def proof_deferred_parameter_blocks(future: Future[int]):
    if future.done():
        def consume(future: Future[int]):
            return future.result()

        return consume(Future())


async def proof_deferred_closure_blocks(future: Future[int]):
    if future.done():
        def consume():
            return future.result()

        future = Future()
        return consume()


async def proof_deferred_local_ok(future: Future[int]):
    if future.done():
        def unused():
            future = Future()

        return future.result()


async def proof_attribute_rebind_blocks(holder: ProofHolder):
    if holder.future.done():
        holder.future = Future()
        return holder.future.result()


async def proof_annotated_attribute_rebind_blocks(holder: ProofHolder):
    if holder.future.done():
        holder.future: Future[int] = Future()
        return holder.future.result()


async def proof_nested_rebind_blocks(future: Future[int], other: Future[int]):
    if future.done():
        if other.done():
            future = Future()
        return future.result()


async def proof_else_keeps_original_ok(future: Future[int], flag: bool):
    if future.done():
        if flag:
            future = Future()
        else:
            return future.result()


async def proof_exiting_rebind_keeps_original_ok(future: Future[int], flag: bool):
    if future.done():
        if flag:
            future = Future()
            return None
        return future.result()


async def proof_walrus_rebind_blocks(future: Future[int]):
    if future.done():
        (future := Future())
        return future.result()


async def proof_test_rebind_blocks(future: Future[int]):
    if future.done() and (future := Future()):
        return future.result()


async def proof_negative_return_ok(future: Future[int]):
    if not future.done():
        return None
    return future.result()


async def proof_negative_else_ok(future: Future[int]):
    if not future.done():
        return None
    else:
        return future.result()


async def proof_negative_ternary_ok(future: Future[int]):
    return None if not future.done() else future.result()


async def proof_negative_or_ok(future: Future[int]):
    return not future.done() or future.result()


async def proof_two_negative_returns_ok(first: Future[int], second: Future[int]):
    if not first.done() or not second.done():
        return None
    return first.result(), second.result()


async def proof_positive_or_blocks(future: Future[int], flag: bool):
    if future.done() or flag:
        return future.result()


async def proof_conditional_return_blocks(future: Future[int], flag: bool):
    if not future.done() and flag:
        return None
    return future.result()


async def proof_fallthrough_blocks(future: Future[int]):
    if future.done():
        pass
    return future.result()


async def proof_unrelated_receiver_blocks(first: Future[int], second: Future[int]):
    if first.done():
        return second.result()


async def proof_for_rebind_blocks(future: Future[int], pending: list[Future[int]]):
    if not future.done():
        return
    for future in pending:
        future.result()


async def proof_while_backedge_blocks(future: Future[int], count: int):
    if not future.done():
        return
    while count:
        future.result()
        future = Future()
        count -= 1


async def proof_zero_iterations_blocks(future: Future[int], count: int):
    while count:
        if not future.done():
            return
        count -= 1
    future.result()


def proof_maybe_raise(flag: bool):
    if flag:
        raise ValueError()


async def proof_exceptional_path_blocks(future: Future[int], flag: bool):
    try:
        proof_maybe_raise(flag)
        if not future.done():
            return
    except ValueError:
        future.result()


async def proof_handler_fallthrough_blocks(future: Future[int], flag: bool):
    try:
        proof_maybe_raise(flag)
    except ValueError:
        if not future.done():
            return
    future.result()


async def proof_finally_blocks(future: Future[int], flag: bool):
    try:
        proof_maybe_raise(flag)
        if not future.done():
            return
    finally:
        future.result()


async def proof_match_sibling_blocks(future: Future[int], mode: int):
    match mode:
        case 0:
            if not future.done():
                return
        case 1:
            future.result()


async def proof_match_binding_blocks(future: Future[int], pending: Future[int]):
    if not future.done():
        return
    match pending:
        case future:
            future.result()


async def proof_suppressed_exception_blocks(future: Future[int], flag: bool):
    from contextlib import suppress

    with suppress(ValueError):
        proof_maybe_raise(flag)
        if not future.done():
            return
    future.result()


async def proof_annotation_only_ok(future: Future[int]):
    if not future.done():
        return
    future: Future[int]
    return future.result()


async def proof_rhs_before_rebind_ok(future: Future[int]):
    if not future.done():
        return
    future = future.result()
    return future


async def proof_comprehension_scope_ok(future: Future[int], pending: list[Future[int]]):
    if not future.done():
        return
    [future for future in pending]
    return future.result()


async def proof_comprehension_binding_blocks(future: Future[int], pending: list[Future[int]]):
    if not future.done():
        return
    return [future.result() for future in pending]


async def proof_comprehension_walrus_blocks(future: Future[int], pending: list[Future[int]]):
    if not future.done():
        return
    [(future := item) for item in pending]
    return future.result()


async def proof_inline_lambda_ok(future: Future[int]):
    if not future.done():
        return
    consume = lambda: future.result()
    return consume()


class ProofInheritedFuture(Future[int]):
    pass


class ProofOverriddenResult(Future[int]):
    def result(self, timeout=None):
        import time

        time.sleep(0.01)


async def proof_inherited_methods_ok(future: ProofInheritedFuture):
    if future.done():
        return future.result()


async def proof_overridden_result_blocks(future: ProofOverriddenResult):
    if future.done():
        return future.result()


async def proof_failed_match_guard_blocks(future: Future[int], pending: Future[int]):
    if not future.done():
        return
    match pending:
        case future if False:
            pass
        case _:
            return future.result()


proof_pending = Future()


async def proof_import_rebind_blocks(future: Future[int]):
    if not future.done():
        return
    from app import proof_pending as future

    return future.result()


import asyncio


class WaitOverriddenException(Future[int]):
    def exception(self, timeout=None):
        import time

        time.sleep(0.01)
        return RuntimeError("custom exception reader")


def wait_make_future() -> Future[int]:
    import time

    time.sleep(0.01)
    return Future()


async def wait_default_exception_blocks(future: Future[int]):
    return future.exception()


async def wait_pending_exception_blocks(future: Future[int]):
    return future.exception(timeout=0.02)


async def wait_inherited_exception_blocks(future: ProofInheritedFuture):
    return future.exception(0.02)


async def wait_rebound_exception_blocks(future: Future[int]):
    if future.done():
        future = Future()
        return future.exception(timeout=0.02)


async def wait_misleading_done_exception_blocks(future: ProofMisleadingFuture):
    if future.done():
        return future.exception(timeout=0.02)


async def wait_custom_exception_blocks(future: WaitOverriddenException):
    if future.done():
        return future.exception(timeout=0)


async def wait_positive_result_blocks(future: Future[int]):
    return future.result(timeout=0.02)


async def wait_dynamic_exception_blocks(future: Future[int], timeout: float | None):
    return future.exception(timeout=timeout)


async def wait_poll_eager_receiver_blocks():
    return wait_make_future().exception(timeout=0)


async def wait_done_exception_ok(future: Future[int]):
    if future.done():
        return future.exception()


async def wait_negative_guard_exception_ok(future: Future[int]):
    if not future.done():
        return None
    return future.exception()


async def wait_poll_result_zero_ok(future: Future[int]):
    return future.result(0)


async def wait_poll_result_float_ok(future: Future[int]):
    return future.result(timeout=0.0)


async def wait_poll_result_negative_ok(future: Future[int]):
    return future.result(-1)


async def wait_poll_result_signed_zero_ok(future: Future[int]):
    return future.result(timeout=-0.0)


async def wait_poll_exception_zero_ok(future: Future[int]):
    return future.exception(timeout=+0)


async def wait_poll_exception_negative_ok(future: Future[int]):
    return future.exception(-0.125)


async def wait_poll_exception_bool_ok(future: Future[int]):
    return future.exception(timeout=False)


async def wait_inherited_poll_ok(future: ProofInheritedFuture):
    try:
        future.result(0)
    except TimeoutError:
        return future.exception(timeout=0)


async def wait_asyncio_exception_ok(future: asyncio.Future[int]):
    return future.exception()


async def wait_unbound_poll_ok(future: Future[int]):
    try:
        Future.result(future, 0)
    except TimeoutError:
        return Future.exception(self=future, timeout=0)


async def wait_unbound_done_exception_ok(future: Future[int]):
    if future.done():
        return Future.exception(future)


async def wait_unbound_exception_blocks(future: Future[int]):
    return Future.exception(future, timeout=0.02)


async def wait_true_timeout_exception_blocks(future: Future[int]):
    return future.exception(timeout=True)
