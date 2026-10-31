"""self.engine sync vs async SQLAlchemy recall."""

from __future__ import annotations

from sqlalchemy import create_engine
from sqlalchemy.ext.asyncio import create_async_engine


class SyncWorker:
    def __init__(self) -> None:
        self.engine = create_engine("sqlite://")

    async def self_begin(self):
        with self.engine.begin() as conn:
            conn.execute("select 1")

    async def self_connect(self):
        with self.engine.connect() as conn:
            conn.execute("select 1")


class AsyncWorker:
    def __init__(self) -> None:
        self.engine = create_async_engine("sqlite+aiosqlite://")

    async def async_begin_ok(self):
        async with self.engine.begin():
            pass

    async def async_connect_ok(self):
        async with self.engine.connect():
            pass
