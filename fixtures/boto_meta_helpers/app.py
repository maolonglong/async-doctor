"""boto3 meta helpers are not IO — must stay clean; get_object still reports."""

from __future__ import annotations

import boto3

s3 = boto3.client("s3")


async def paginator_waiter_ok():
    _ = s3.get_paginator("list_objects_v2")
    _ = s3.get_waiter("object_exists")
    _ = s3.can_paginate("list_objects_v2")
    _ = s3.generate_presigned_url("get_object", Params={"Bucket": "b", "Key": "k"})
    _ = s3.meta


async def get_object_blocks():
    s3.get_object(Bucket="b", Key="k")
