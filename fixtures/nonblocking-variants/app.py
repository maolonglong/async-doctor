import queue
import threading

import httpx
import requests

_lock = threading.Lock()
_sem = threading.Semaphore(4)
_q: "queue.Queue[int]" = queue.Queue()          # unbounded
_bq: "queue.Queue[int]" = queue.Queue(maxsize=1)


async def nb_sem() -> None:
    if _sem.acquire(blocking=False):   # FP expected: non-blocking
        _sem.release()


async def nb_lock() -> None:
    if _lock.acquire(False):           # FP expected: non-blocking (positional)
        _lock.release()


async def nb_join(t: threading.Thread) -> None:
    t.join(timeout=0)                  # FP expected: zero timeout


async def unbounded_put() -> None:
    _q.put(1)                          # FP expected: unbounded queue never blocks
    _q.put_nowait(2)                   # FP expected


async def bounded_put() -> None:
    _bq.put(1)                         # TP expected


async def ctor_only() -> None:
    c = httpx.Client()                 # FP expected: constructor, no IO
    s = requests.Session()             # FP expected: constructor, no IO
    del c, s


async def with_lock() -> None:
    with _lock:                        # debatable: short critical section
        pass
