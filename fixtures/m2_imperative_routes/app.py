"""Imperative FastAPI/Starlette-style route registration."""
import time

from routes import router


class App:
    def __init__(self):
        self.routes = []

    def add_api_route(self, path, endpoint, **kwargs):
        self.routes.append((path, endpoint))

    def add_route(self, path, endpoint, **kwargs):
        self.routes.append((path, endpoint))

    def include_router(self, r):
        self.routes.extend(getattr(r, "routes", []))


class Route:
    def __init__(self, path, endpoint, **kwargs):
        self.path = path
        self.endpoint = endpoint


class WebSocketRoute:
    def __init__(self, path, endpoint, **kwargs):
        self.path = path
        self.endpoint = endpoint


app = App()


async def via_add_api_route():
    time.sleep(1)


async def via_add_route():
    time.sleep(1)


async def via_route_ctor():
    time.sleep(1)


async def via_ws_route():
    time.sleep(1)


app.add_api_route("/a", via_add_api_route)
app.add_route("/b", via_add_route)
_routes = [
    Route("/c", via_route_ctor),
    WebSocketRoute("/ws", via_ws_route),
]
app.include_router(router)
