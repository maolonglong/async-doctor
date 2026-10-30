"""Production entry used with open+read merge fixture."""


async def handler():
    f = open("x")
    _ = f.read()
