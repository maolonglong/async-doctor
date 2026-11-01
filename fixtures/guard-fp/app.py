import asyncio, threading

def _run_in_thread(coro):
    out = {}
    t = threading.Thread(target=lambda: out.setdefault("r", asyncio.run(coro)))
    t.start(); t.join()
    return out["r"]

def run_sync(coro):
    try:
        loop = asyncio.get_running_loop()
    except RuntimeError:
        loop = None
    if loop is not None:
        return _run_in_thread(coro)
    return asyncio.run(coro)          # 16: unreachable while a loop runs

async def work():
    return 1

async def handler():
    return run_sync(work())
