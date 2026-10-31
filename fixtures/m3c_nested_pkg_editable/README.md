# m3c_nested_pkg_editable

Same code as `m3c_nested_pkg_flat`, but `libcore` lives at `libcore/libcore/`
(nested layout) and is exposed via:

- `pyrightconfig.json` / `[tool.ruff] src` / `[tool.ty.environment] extra-paths`
- setuptools editable finder `MAPPING` under `.venv` (import-hook `.pth`, not
  a path-style editable)

Expect the same **4 findings** as the flat fixture. Without search-path
discovery this used to report nothing (and no unresolved candidate).
