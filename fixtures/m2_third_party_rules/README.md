# m2_third_party_rules fixtures

Clean-room stub packages under `.venv/.../site-packages` for async-doctor M2 tests.

These are **not** copies of real third-party libraries. They exist so type resolution
and the built-in third-party rule table can see External qualnames (`requests`,
`httpx`, `redis`, `sqlalchemy`, etc.) without installing actual dependencies.
