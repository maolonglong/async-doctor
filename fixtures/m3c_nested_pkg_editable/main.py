"""Scan-root entries importing the flat libcore package."""

from __future__ import annotations

from libcore.cfg import load_config


async def handler():
    return load_config()


async def other():
    return load_config()
