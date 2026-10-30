"""Negative fixture: time.sleep in a sync def must not be reported."""

import time


def sync_helper():
    time.sleep(1)
