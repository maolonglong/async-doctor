"""aioboto3 Session.client must not be treated as sync boto3 (0 findings)."""

import aioboto3


async def aio_session_client():
    session = aioboto3.Session()
    async with session.client("s3") as s3:
        await s3.get_object(Bucket="b", Key="k")


async def aio_inline():
    async with aioboto3.Session().client("s3") as s3:
        await s3.list_objects_v2(Bucket="b")
