"""Sync store used from async entry (clean-room)."""

from __future__ import annotations

import sqlite3


class Store:
    def save(self, q: str) -> None:
        conn = sqlite3.connect(":memory:")
        conn.execute("insert into t values (?)", (q,))
