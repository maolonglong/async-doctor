"""Optional/union receivers narrowed by isinstance / is not None."""

from __future__ import annotations

import time


class SyncClient:
    def fetch(self) -> None:
        time.sleep(1)


class AsyncClient:
    async def fetch(self) -> None:
        return None


async def narrowed_not_none(client: SyncClient | None):
    if client is not None:
        client.fetch()


async def narrowed_isinstance(client: SyncClient | AsyncClient):
    if isinstance(client, SyncClient):
        client.fetch()


async def not_narrowed_ok(client: SyncClient | AsyncClient):
    # Negative: without narrowing we must not guess SyncClient (FP worse than miss).
    # If unresolved, no finding; if ty picks one arm, still prefer no sync sink guess.
    _ = client


async def none_branch_ok(client: SyncClient | None):
    if client is None:
        return
    # After early return on None, subsequent code is narrowed — but this body is empty.
