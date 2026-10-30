"""Regression: entry-local param guards stay findings (async-branch)."""

from pathlib import Path


async def handler(flag):
    if flag:
        Path("x").read_text()
