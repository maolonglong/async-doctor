"""boto3 dynamic client methods must report with aioboto3 advice."""

import boto3


async def a():
    s3 = boto3.client("s3")
    s3.get_object(Bucket="b", Key="k")


async def resource_table():
    table = boto3.resource("dynamodb").Table("t")
    table.get_item(Key={"id": 1})
