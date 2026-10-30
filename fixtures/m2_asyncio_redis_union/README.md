# m2_asyncio_redis_union

Union-typed asyncio redis receivers (`Pipeline | ClusterPipeline`,
`Redis | RedisCluster`) must not fall back to mixin `CoreCommands.set`
normalized as sync `redis.Redis.set`.
