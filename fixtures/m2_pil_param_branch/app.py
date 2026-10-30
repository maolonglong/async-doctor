import io
from PIL import Image


async def use_image_open():
    # Proven in-memory buffer — must not be a finding or blocking-file candidate.
    Image.open(io.BytesIO(b"\x00"))


async def use_image_save():
    image = Image.Image()
    buf = io.BytesIO()
    image.save(buf, format="PNG")


async def use_image_open_path():
    # Path string behind PIL's `if is_path(fp):` guard — downgrade to candidate.
    Image.open("x.png")


async def use_image_save_path():
    image = Image.Image()
    image.save("out.png", format="PNG")
