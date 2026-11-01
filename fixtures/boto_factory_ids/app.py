"""Two boto factory sites that share sink key `boto3.client` must both emit."""

import boto3


def two_sites_client_then_session(profile):
    a = boto3.client("rds")
    b = boto3.Session(profile_name=profile).client("rds")
    return a, b


def two_sites_session_then_client(profile):
    b = boto3.Session(profile_name=profile).client("rds")
    a = boto3.client("rds")
    return a, b


def two_sites_bound_session(profile):
    a = boto3.client("rds")
    sess = boto3.Session(profile_name=profile)
    b = sess.client("rds")
    return a, b


async def e1_client_then_session(p):
    return two_sites_client_then_session(p)


async def e1_session_then_client(p):
    return two_sites_session_then_client(p)


async def e1_bound_session(p):
    return two_sites_bound_session(p)


async def available_regions_ok():
    # Local metadata helper — must not be flagged as blocking-http.
    _ = boto3.Session().get_available_regions("s3")
