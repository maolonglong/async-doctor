# Third-party notices

This file lists third-party material that ships inside the `async-doctor`
binary or is otherwise redistributed with this project.

## ruff / ty (MIT)

async-doctor links Astral's published crates (`ruff_db`, `ruff_python_ast`,
`ruff_python_parser`, `ty_python_semantic`, `ty_python_core`,
`ty_module_resolver`, `ty_site_packages`, `ty_vendored`, and related packages).

- License: MIT
- Copyright (c) 2022 Charles Marsh
- Source: <https://github.com/astral-sh/ruff>

The salsa database in `crates/async_doctor_ty` follows the public `TestDb`
pattern from `ty_python_semantic` (same license and copyright).

## typeshed (Apache-2.0)

`ty_vendored` embeds [typeshed](https://github.com/python/typeshed) stubs for
the Python standard library.

- License: Apache License 2.0
- Copyright: the typeshed contributors
- License text: <https://github.com/python/typeshed/blob/main/LICENSE>

## Fixture stub packages (tests only)

`fixtures/m2_third_party_rules/.venv` contains **clean-room** minimal stubs of
third-party APIs (`requests`, `httpx`, `redis`, `sqlalchemy`, …) used only so
ty can resolve types in tests. They are not copies of those projects' source
and are not shipped in the `async-doctor` binary.

## Design provenance

Architecture ideas (call-graph reachability, offload cuts, agent-first JSON)
were derived from public docs and the project's own design canvas. **No code**
was copied from hydroid, strato, or react-doctor.

## Other Rust dependencies

Remaining crates are recorded in `Cargo.lock`. Most are MIT and/or Apache-2.0;
run `cargo license` for a machine-readable inventory.
