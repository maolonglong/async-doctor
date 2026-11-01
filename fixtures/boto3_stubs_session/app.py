"""boto3-stubs Session.client must still flag sync ops; aioboto3 stays clean."""

import aioboto3
import boto3


async def stubs_session_client_get_object():
    s3 = boto3.session.Session().client("s3")
    s3.get_object(Bucket="b", Key="k")


async def stubs_bound_session_put_object():
    sess = boto3.Session()
    s3 = sess.client("s3")
    s3.put_object(Bucket="b", Key="k", Body=b"x")


async def aio_session_client_ok():
    session = aioboto3.Session()
    async with session.client("s3") as s3:
        await s3.get_object(Bucket="b", Key="k")


async def aio_inline_ok():
    async with aioboto3.Session().client("s3") as s3:
        await s3.list_objects_v2(Bucket="b")
