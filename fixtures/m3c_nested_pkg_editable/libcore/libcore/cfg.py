"""Config loader with sync HTTP + sleep backoff (clean-room)."""

from __future__ import annotations

import time

import requests


def _fetch(key: str) -> str:
    for attempt in range(3):
        try:
            return requests.get(f"https://config.example.com/{key}", timeout=5).text
        except Exception:
            time.sleep(0.5 * 2**attempt)
    return ""


def load_config() -> dict:
    return {"secret": _fetch("secrets")}
