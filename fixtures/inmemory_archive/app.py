"""zipfile/tarfile on BytesIO must stay clean; path archives still report."""

from __future__ import annotations

import io
import zipfile
import tarfile


async def zip_bytesio_ok():
    buf = io.BytesIO()
    with zipfile.ZipFile(buf, "w") as zf:
        zf.writestr("a.txt", "hi")
        zf.writestr("b.txt", "yo")


async def zip_bytesio_inline_ok():
    zf = zipfile.ZipFile(io.BytesIO(), "w")
    zf.writestr("a.txt", "hi")


async def tar_bytesio_ok():
    buf = io.BytesIO()
    # tarfile.open on a stream (mode w:gz needs a real file often; use w:).
    with tarfile.open(fileobj=buf, mode="w") as tf:
        pass


async def zip_path_blocks():
    with zipfile.ZipFile("out.zip", "w") as zf:
        zf.writestr("a.txt", "hi")
        zf.writestr("b.txt", "yo")


async def ctor_sendmail_one_finding():
    import smtplib

    smtplib.SMTP("localhost").sendmail("a", ["b"], "c")
