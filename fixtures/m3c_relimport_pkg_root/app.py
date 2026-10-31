from .utils import helper
from .ns_pkg.thing import value


async def run():
    return helper() or value
