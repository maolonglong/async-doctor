# m2_redis_pipeline_real

Clean-room stubs that mirror the real redis 5.x class hierarchy:

- `redis.commands.core.CoreCommands` mixin defines `set`/`get`
- `redis.client.Redis(CoreCommands)` and `Pipeline(Redis)`
- `redis.asyncio.client.Pipeline` with `async def execute`

Synthetic `Pipeline.set` qualnames alone hid the mixin-resolution bug.
