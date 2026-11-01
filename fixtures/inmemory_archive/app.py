"""zipfile/tarfile on BytesIO: pure in-memory ops clean; disk-touching still report."""

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
    with tarfile.open(fileobj=buf, mode="w") as tf:
        pass


async def zip_bytesio_write_path_blocks():
    zipfile.ZipFile(io.BytesIO(), "w").write("disk.txt")


async def zip_bytesio_extractall_blocks():
    zipfile.ZipFile(io.BytesIO(b"PK\x05\x06" + b"\x00" * 18)).extractall("/tmp/out")


async def tar_bytesio_add_blocks():
    tarfile.open(fileobj=io.BytesIO(), mode="w").add("disk.txt")


async def zip_path_blocks():
    with zipfile.ZipFile("out.zip", "w") as zf:
        zf.writestr("a.txt", "hi")
        zf.writestr("b.txt", "yo")


async def ctor_sendmail_one_finding():
    import smtplib

    smtplib.SMTP("localhost").sendmail("a", ["b"], "c")
