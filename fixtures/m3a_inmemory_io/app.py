"""In-memory buffers must not be flagged as blocking file IO."""

from __future__ import annotations

import io
from pathlib import Path

from PIL import Image


async def image_open_bytesio_ok():
    Image.open(io.BytesIO(b"\x00"))


async def image_save_bytesio_ok():
    image = Image.Image()
    buf = io.BytesIO()
    image.save(buf, format="PNG")


async def open_path_blocks():
    # Positive control: real path open still reports.
    open("x.txt")


async def path_read_blocks():
    Path("x.txt").read_text()
