"""Call path through helpers that themselves contain sinks.

Expected: `handler` reports blocking-file for both `_check` and `_patch`
open/read/write sites (chains pass through `ensure`). Open+read/write for the
same with-block may merge; `_check` and `_patch` paths must not mash together.
"""


class P:
    def _check(self, p):
        with open(p) as f:
            return f.read()

    def _patch(self, p):
        with open(p) as f:
            data = f.read()
        with open(p, "w") as f:
            f.write(data)

    def ensure(self, p):
        self._check(p)
        self._patch(p)


async def handler(p):
    P().ensure(p)
