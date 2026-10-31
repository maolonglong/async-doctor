import asyncio
import anthropic


class AP:
    def __init__(self, api_key: str, model: str) -> None:
        self._client = anthropic.Anthropic(api_key=api_key)
        self._model = model

    async def r1_try(self, user: str) -> str:
        try:
            message = await asyncio.to_thread(
                self._client.messages.create,
                model=self._model,
                max_tokens=10,
                messages=[{"role": "user", "content": user}],
            )
        except Exception as exc:
            raise RuntimeError("x") from exc
        return str(message)

    async def r2_notry(self, user: str) -> str:
        message = await asyncio.to_thread(
            self._client.messages.create,
            model=self._model,
            max_tokens=10,
            messages=[{"role": "user", "content": user}],
        )
        return str(message)
