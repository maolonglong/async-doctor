"""Higher-order callable args: propagate through the argument actually passed."""
import time


def block():
    time.sleep(1)


def apply(fn):
    fn()


async def handler():
    apply(block)
