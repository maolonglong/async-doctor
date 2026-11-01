# Clean-room implementation on ty/ruff, MIT license

Status: accepted

## Problem

The tool needs typed Python semantics and a packaging story without inheriting
license or training-use restrictions from nearby agent-linter projects, and
without taking an unbounded dependency on churning Astral internals.

## Decision

async-doctor is a clean-room MIT project. Implementation may read public docs
and public crates (including how to use hydroid/ruff/ty crates) but must not
copy code from react-doctor (license forbids AI training/eval use) or from
unlicensed sources such as strato. All ty/ruff calls live in the single adapter
crate `async_doctor_ty`, pinned with exact `=` versions and bumped together in
dedicated PRs after benchmarks. typeshed ships via `ty_vendored` (Apache-2.0
notice in `THIRD_PARTY_NOTICES.md`). Packaging uses maturin `bindings = "bin"`
so `uvx async-doctor` works without a Rust toolchain on the user machine.

## Alternatives considered

- **Derive from react-doctor / strato source.** Rejected: license and missing
  license block clean reuse; clean-room keeps the MIT surface clear.
- **Loose semver ranges on ty/ruff.** Rejected: those crates are internal
  components with no semver promise; exact pins contain churn.
- *(Inferred, not in the original decision record.)* Reimplement Python
  semantics without ty — out of scope given typed receivers and site-packages
  resolution are the product.

## Consequences

Adapter upgrades are explicit PRs with benchmarks. Agents must not paste
restricted upstream code into this tree. Before open-sourcing, vendored
dev-skill license gaps (notably `fast-rust`) must be resolved separately.
