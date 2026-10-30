"""Regression: module-const prune only when ty binds to unreassigned module symbol."""

import time

DEBUG = False


async def local_reassign():
    DEBUG = True
    if DEBUG:
        time.sleep(1)


async def param_shadow(DEBUG):
    if DEBUG:
        time.sleep(1)


def configure():
    global DEBUG
    DEBUG = True


async def after_global():
    if DEBUG:
        time.sleep(1)
