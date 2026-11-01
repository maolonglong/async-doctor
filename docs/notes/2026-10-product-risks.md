# Product risks and mitigations

Design-era risks that still shape how agents change the analyzer. Prefer the
mitigation over inventing a new one.

## Symptom

Without these constraints, agents re-propose loose ty pins, name-based
guessing for dynamic calls, or scanning all of site-packages.

## Cause

Static analysis on top of internal Astral crates plus large third-party trees
has recurring failure modes that tests alone do not name.

## Fix or workaround

| Risk | Mitigation |
| --- | --- |
| ty API churn | Exact `=` pins; single adapter crate `async_doctor_ty`; bump in a dedicated PR after benchmarks |
| Dynamic dispatch | Mark unresolved → `candidates` only; never guess a finding by name |
| C extensions without source | Rule table by type; unknown natives stay unreported |
| Large site-packages | Follow reachable functions only; cache by package version / content fingerprint |
| False positives | Findings require proof; each FP becomes a regression fixture |

## Guardrail

Standing orders in `AGENTS.md` (clean-room / prove-only findings) and
[`../decisions/2026-10-30-reporting-contract.md`](../decisions/2026-10-30-reporting-contract.md).
CI: `cargo test --workspace` plus fixture regressions for each fixed FP.
