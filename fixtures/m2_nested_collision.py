"""Nested functions with the same name in different enclosers must not merge."""
import time


def outer_a():
    def inner():
        time.sleep(1)

    inner()


def outer_b():
    def inner():
        return 0

    inner()


async def handler_a():
    outer_a()


async def handler_b():
    # Must NOT report: outer_b.inner is clean.
    outer_b()
