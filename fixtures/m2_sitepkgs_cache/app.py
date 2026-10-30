"""Project code that calls a followable third-party helper."""

from libx import fetch


async def handler():
    fetch("https://example.com")
