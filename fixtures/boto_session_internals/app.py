"""botocore.session internals must not be blocking-http; real client ops must."""

import boto3
from botocore.session import get_session


async def regions_ok():
    sess = boto3.Session(region_name="us-east-1")
    return sess.get_available_regions("s3")


async def service_ops_ok():
    return get_session().get_service_model("s3").operation_names


async def client_get_object_blocks():
    s3 = boto3.client("s3")
    return s3.get_object(Bucket="b", Key="k")


async def session_client_describe_blocks():
    return boto3.Session().client("rds").describe_db_instances()
