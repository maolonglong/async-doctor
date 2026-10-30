"""Negative fixture: time.sleep under if TYPE_CHECKING must not be reported."""

import time
from typing import TYPE_CHECKING


async def handler():
    if TYPE_CHECKING:
        time.sleep(1)
