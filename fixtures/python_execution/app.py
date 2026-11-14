"""Execution-location and coroutine/protocol regression cases."""

import asyncio
import concurrent.futures
import functools
import time


def block():
    time.sleep(0.001)


async def get_rows():
    return [3, 7]


class Resource:
    def __enter__(self):
        return self

    def __exit__(self, *exc):
        return False


async def get_resource():
    return Resource()


async def coroutine_sync_for():
    return [row for row in get_rows()]


async def coroutine_async_for():
    return [row async for row in get_rows()]


async def coroutine_named_for():
    rows = get_rows()
    for row in rows:
        pass


async def coroutine_sync_with():
    with get_resource():
        pass


async def coroutine_async_with():
    async with get_resource():
        pass


async def coroutine_awaited_ok():
    rows = [row for row in await get_rows()]
    with await get_resource():
        return rows


def pool_with_preamble(fn):
    block()
    return asyncio.to_thread(fn)


def pool_with_postamble(fn):
    future = asyncio.get_running_loop().run_in_executor(None, fn)
    block()
    return future


def pool_only(fn):
    return asyncio.to_thread(fn)


async def wrapper_preamble():
    await pool_with_preamble(block)


async def wrapper_postamble():
    await pool_with_postamble(block)


async def wrapper_only_ok():
    await pool_only(block)


def generate(flag=True):
    if flag:
        block()
    yield 3
    yield 7


async def generator_named():
    rows = generate()
    return list(rows)


async def generator_alias():
    rows = generate()
    alias = rows
    return [row for row in alias]


async def generator_created_offloop():
    rows = await asyncio.to_thread(generate)
    return list(rows)


async def generator_for():
    rows = generate()
    for row in rows:
        pass


async def generator_creation_only_ok():
    rows = generate()
    return rows


async def generator_consumed_offloop_ok():
    rows = generate()
    return await asyncio.to_thread(list, rows)


async def generator_rebound_ok():
    rows = generate()
    rows = [11, 13]
    return list(rows)


async def generator_guard_false_ok():
    rows = generate(False)
    return list(rows)


async def generator_offloop_guard_false_ok():
    rows = await asyncio.to_thread(generate, False)
    return list(rows)


async def generator_alias_after_rebind():
    rows = generate()
    alias = rows
    rows = [11, 13]
    return list(alias)


async def generator_exhausted_on_worker_ok():
    rows = generate()
    await asyncio.to_thread(list, rows)
    return list(rows)


def generate_late():
    yield 3
    block()
    yield 7


async def generator_partial_worker_consumption():
    rows = generate_late()
    await asyncio.to_thread(next, rows)
    return list(rows)


async def generator_ambiguous_source_unknown(flag):
    if flag:
        rows = generate()
    else:
        rows = [11, 13]
    return list(rows)


async def generator_shadowed_consumer_ok():
    def list(rows):
        return rows

    rows = generate()
    return list(rows)


def consume_forwarded(flag):
    rows = generate(flag)
    return list(rows)


async def generator_forwarded_true():
    return consume_forwarded(True)


async def generator_forwarded_false_ok():
    return consume_forwarded(False)


async def callback_soon():
    loop = asyncio.get_running_loop()
    loop.call_soon(block)


async def callback_later_partial():
    loop = asyncio.get_running_loop()
    loop.call_later(0, functools.partial(block))


async def callback_threadsafe():
    loop = asyncio.get_running_loop()
    loop.call_soon_threadsafe(block)


async def callback_future():
    future = asyncio.get_running_loop().create_future()
    future.add_done_callback(lambda _: block())
    future.set_result(17)


def guarded_block(enabled):
    if enabled:
        block()


async def callback_argument_true():
    asyncio.get_running_loop().call_soon(guarded_block, True)


async def callback_argument_false_ok():
    asyncio.get_running_loop().call_soon(guarded_block, False)


async def callback_at():
    loop = asyncio.get_running_loop()
    loop.call_at(loop.time(), block)


async def callback_external():
    asyncio.get_running_loop().call_soon(time.sleep, 0.001)


async def callback_partial_argument_true():
    asyncio.get_running_loop().call_soon(functools.partial(guarded_block, True))


async def callback_partial_argument_false_ok():
    asyncio.get_running_loop().call_soon(functools.partial(guarded_block, False))


async def callback_partial_keyword_false_ok():
    asyncio.get_running_loop().call_soon(functools.partial(guarded_block, enabled=False))


async def callback_lambda_argument_true():
    asyncio.get_running_loop().call_soon(lambda enabled: guarded_block(enabled), True)


async def callback_lambda_argument_false_ok():
    asyncio.get_running_loop().call_soon(lambda enabled: guarded_block(enabled), False)


async def callback_keyword():
    asyncio.get_running_loop().call_soon(callback=block)


class CallbackOwner:
    def invoke(self, enabled):
        guarded_block(enabled)


async def callback_bound_method():
    asyncio.get_running_loop().call_soon(CallbackOwner().invoke, True)


async def callback_bound_method_false_ok():
    asyncio.get_running_loop().call_soon(CallbackOwner().invoke, False)


async def callback_task():
    task = asyncio.create_task(get_rows())
    task.add_done_callback(lambda _: block())
    await task


def on_done(enabled, future):
    guarded_block(enabled)


async def callback_future_partial():
    future = asyncio.get_running_loop().create_future()
    future.add_done_callback(functools.partial(on_done, True))
    future.set_result(17)


async def callback_future_partial_false_ok():
    future = asyncio.get_running_loop().create_future()
    future.add_done_callback(functools.partial(on_done, False))
    future.set_result(17)


def rebound_parameter(enabled):
    enabled = True
    guarded_block(enabled)


async def callback_rebound_parameter():
    asyncio.get_running_loop().call_soon(rebound_parameter, False)


async def callback_nested_partial_false_ok():
    asyncio.get_running_loop().call_soon(
        functools.partial(functools.partial(guarded_block, enabled=True), enabled=False)
    )


async def callback_partial_keyword_true():
    asyncio.get_running_loop().call_soon(functools.partial(guarded_block, enabled=True))


async def callback_generator_lambda_ok():
    asyncio.get_running_loop().call_soon(lambda: (yield block()))


async def deferred_coroutine():
    block()


async def deferred_async_generator():
    block()
    yield 17


async def callback_deferred_bodies_ok():
    loop = asyncio.get_running_loop()
    loop.call_soon(deferred_coroutine)
    loop.call_soon(deferred_async_generator)
    loop.call_soon(generate)


async def callback_worker_future_ok():
    with concurrent.futures.ThreadPoolExecutor() as pool:
        future = pool.submit(lambda: 17)
        future.add_done_callback(lambda _: block())


class NotAnEventLoop:
    def call_soon(self, callback):
        return callback


async def callback_unrelated_ok():
    NotAnEventLoop().call_soon(block)


class SlowEnter(Resource):
    def __enter__(self):
        block()
        return self


class SlowExit(Resource):
    def __exit__(self, *exc):
        block()
        return False


class InheritedEnter(SlowEnter):
    pass


async def context_enter_inline():
    with SlowEnter():
        pass


async def context_enter_named():
    resource = SlowEnter()
    with resource:
        pass


async def context_exit_inline():
    with SlowExit():
        pass


async def context_exit_named():
    resource = SlowExit()
    with resource:
        pass


async def context_enter_inherited():
    with InheritedEnter():
        pass


def consume_context():
    with SlowEnter():
        pass


async def context_offloaded_ok():
    await asyncio.to_thread(consume_context)


class LeftEnter(SlowEnter):
    pass


class RightEnter(SlowEnter):
    def __enter__(self):
        return self


class DiamondEnter(LeftEnter, RightEnter):
    pass


async def context_mro_ok():
    with DiamondEnter():
        pass


async def context_instance_attribute_ok():
    resource = Resource()
    resource.__enter__ = block
    with resource:
        pass


class LazyEnter(Resource):
    def __enter__(self):
        block()
        yield self


async def context_generator_enter_ok():
    with LazyEnter():
        pass


class AsyncResource:
    def __aenter__(self):
        block()
        return asyncio.sleep(0, result=self)

    async def __aexit__(self, *exc):
        return False


async def context_async_enter():
    async with AsyncResource():
        pass


async def returns_generator_lambda():
    return lambda: (yield 17)


async def coroutine_nested_generator_lambda():
    await asyncio.to_thread(returns_generator_lambda)
