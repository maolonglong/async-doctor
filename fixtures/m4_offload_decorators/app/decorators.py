"""Offload decorators: sync_to_async / database_sync_to_async / project wrappers."""
import asyncio
import functools
import time

from asgiref.sync import sync_to_async
from channels.db import database_sync_to_async


@sync_to_async
def bare_offloaded():
    time.sleep(1)


@sync_to_async(thread_sensitive=False)
def called_offloaded():
    time.sleep(1)


@database_sync_to_async
def db_offloaded():
    time.sleep(1)


def _offload(fn, *args, **kw):
    # Inferred offload wrapper: passes a callable param into a real primitive.
    return asyncio.get_event_loop().run_in_executor(None, fn, *args)


def _project_offload(fn):
    @functools.wraps(fn)
    async def wrapper(*a, **kw):
        return await asyncio.to_thread(fn, *a, **kw)

    return wrapper


def _project_offload_via_wrapper(fn):
    @functools.wraps(fn)
    def wrapper(*a, **kw):
        return _offload(fn, *a, **kw)

    return wrapper


@_project_offload
def project_offloaded():
    time.sleep(1)


@_project_offload_via_wrapper
def project_offloaded_indirect():
    time.sleep(1)


def sync_to_async_bogus(fn):
    # Same *name* as asgiref.sync.sync_to_async but does not offload.
    return fn


@sync_to_async_bogus
def not_offloaded_fake_name():
    time.sleep(1)


def _retry(fn):
    # Unknown decorator that just forwards — stays transparent.
    @functools.wraps(fn)
    def wrapper(*a, **kw):
        return fn(*a, **kw)

    return wrapper


@_retry
def retried_blocking():
    time.sleep(1)


async def via_bare():
    await bare_offloaded()


async def via_called():
    await called_offloaded()


async def via_db():
    await db_offloaded()


async def via_project():
    await project_offloaded()


async def via_project_indirect():
    await project_offloaded_indirect()


async def via_fake_name():
    await not_offloaded_fake_name()


async def via_retry():
    await retried_blocking()
