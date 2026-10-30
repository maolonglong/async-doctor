import time


class APIRouter:
    def __init__(self):
        self.routes = []

    def add_api_route(self, path, endpoint, **kwargs):
        self.routes.append((path, endpoint))


router = APIRouter()


async def via_router_add():
    time.sleep(1)


router.add_api_route("/r", via_router_add)
