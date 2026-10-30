# Agent guidelines

Read and follow the project skills in `.cursor/skills/`:

- `fast-rust` for all Rust code.
- `codebase-design` when adding or restructuring modules: deep modules, small interfaces, seams placed where something actually varies.

## Keep files small

- Aim for source files under about 500 lines. Treat 800 lines as a hard ceiling for new or edited files.
- When a file grows past that, split it by responsibility into a module directory (for example `extract/` with one file per concern), not by arbitrary line ranges. Keep the public interface of the module unchanged while splitting.
- Large test suites go in separate files per area under `tests/` or a `tests/` module directory, not one growing `tests.rs`.
- Fixtures and generated data are exempt.

## Ignored / stress tests

Default `cargo test` stays fast for CI. Longer stress loops are marked `#[ignore]`.

- Concurrent cold-cache race stress (`concurrent_cache`):

  ```bash
  cargo test -p async_doctor --test concurrent_cache -- --ignored
  ```

  CI runs a short N≈15–20 variant. Children are spawned in parallel (not sequential
  `.output()`). In debug builds the test passes an internal env to children that
  widens the sticky-miss window: with the fix reverted the CI-sized large test
  failed **10/10** debug runs (~1s each); with the fix it passed **10/10**
  (~10.5s each). That hook is compiled inert in release. The ignored N=100
  stress is for local hunting without relying on the debug hook alone.
