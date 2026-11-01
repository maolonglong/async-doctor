import redis
from sqlalchemy.ext.asyncio import AsyncSession
from sqlalchemy.orm import Session


async def add_only(db: AsyncSession, obj) -> None:
    db.add(obj)            # FP expected: AsyncSession.add is sync, no IO
    await db.commit()      # fine (async)


async def sync_session_add(s: Session, obj) -> None:
    s.add(obj)             # FP expected: Session.add does no IO
    s.expunge(obj)         # FP expected: no IO


async def make_client() -> redis.Redis:
    return redis.Redis.from_url("redis://localhost")  # FP expected: constructor, no IO


async def real_io(s: Session) -> None:
    s.commit()             # TP expected
