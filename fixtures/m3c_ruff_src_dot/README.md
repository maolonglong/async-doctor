# m3c_ruff_src_dot

`[tool.ruff] src = ["."]` must not be re-added as a front-loaded search path.
Expect **2** blocking-db findings (`sqlite3.connect` + `execute`) via
`from bot.store import Store`.
