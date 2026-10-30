"""Higher-order kwargs, bound methods, and functools.partial."""

from __future__ import annotations

import time
from functools import partial


def block() -> None:
    time.sleep(1)


class Worker:
    def block(self) -> None:
        time.sleep(1)


def apply(fn):
    fn()


def apply_kw(*, fn):
    fn()


async def positional_ok():
    apply(block)


async def keyword_fn():
    apply_kw(fn=block)


async def keyword_partial():
    apply_kw(fn=partial(block))


async def bound_method_positional():
    w = Worker()
    apply(w.block)


async def bound_method_keyword():
    w = Worker()
    apply_kw(fn=w.block)


async def safe_no_call():
    # Negative: higher-order receives a non-blocking lambda.
    apply_kw(fn=lambda: None)
