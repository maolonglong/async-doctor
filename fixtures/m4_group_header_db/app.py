import sqlite3
import time


async def via_exec():
    sqlite3.connect("x").execute("select 1")


async def via_connect():
    sqlite3.connect("y")


async def via_sleep():
    time.sleep(1)
