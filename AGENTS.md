# Agent guidelines

Read and follow the project skills in `.cursor/skills/`:

- `fast-rust` for all Rust code.
- `codebase-design` when adding or restructuring modules: deep modules, small interfaces, seams placed where something actually varies.

## Keep files small

- Aim for source files under about 500 lines. Treat 800 lines as a hard ceiling for new or edited files.
- When a file grows past that, split it by responsibility into a module directory (for example `extract/` with one file per concern), not by arbitrary line ranges. Keep the public interface of the module unchanged while splitting.
- Large test suites go in separate files per area under `tests/` or a `tests/` module directory, not one growing `tests.rs`.
- Fixtures and generated data are exempt.
