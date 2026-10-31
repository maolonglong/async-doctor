"""Async entry importing a same-tree package via absolute import."""

from __future__ import annotations

from bot.store import Store


async def handle(q: str):
    Store().save(q)
