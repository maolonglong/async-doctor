import io
from PIL import Image

async def use_image_open():
    # Module-level open under PIL.Image (path-guarded).
    Image.open(io.BytesIO(b"\x00"))

async def use_image_save():
    image = Image.Image()
    buf = io.BytesIO()
    image.save(buf, format="PNG")
