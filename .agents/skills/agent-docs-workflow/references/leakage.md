# Process leakage

Adapted from the `dsh-trim-cot-leakage` skill in [deepseek-ai/deepseek-harness](https://github.com/deepseek-ai/deepseek-harness) (MIT License, Copyright (c) 2026 DeepSeek).

Process leakage is prose written from the authoring session's vantage instead of the repository's. It cites things only that session could see, narrates the change instead of the state, or argues with a reviewer who has left. It shows up in docs, code comments, docstrings, test names, and file names.

## The test

For each suspect passage, ask: could a reader at HEAD, with no access to any chat, PR thread, or uncommitted draft, resolve every reference and verify every claim? If not, restate the surviving facts from the repository's vantage and delete the rest. On current-state docs (README, `AGENTS.md`, `ARCHITECTURE.md`, topic docs), a change story fails even when it resolves; it belongs in git history, a completed plan, or a decision.

## Categories

1. **Dead session citations**: `(decision 7)`, `(audit C2)`, `design §4`, `plan §1.4`, milestone or phase codes (`M2`, `T4`, `W3`, `P1`) in code, comments, or file names. If a committed doc owns the reference, cite it by path. Otherwise delete the citation and restate its fact so it stands alone. A file named after a milestone (`m2.rs`) gets a name that says what it holds.
2. **PR and stack vantage**: "this PR adds", "a later PR", "the previous commit", "this branch". State the shipped mechanism; deferred work becomes a `TODO` with an issue link.
3. **Change narration and version stamps**: "used to", "no longer", "previously", "the old X", "was renamed", "now" contrasting with a past state, "today", "this cut", "v1" as a stamp. State present behaviour. A fixed regression becomes a present-tense counterfactual ("without X, Y happens").
4. **Review choreography**: "rejected in review", "the reviewer confirmed", "round 2", "v5 of this note". Keep the decision and its reason as plain fact; drop who said it when.
5. **Reviewer-addressed justification**: "this is correct because…", "the cast is safe, it simply…". State the invariant that makes the code safe, or delete the comment if the code shows it.
6. **Derivation transcripts**: control-flow narration ("first we X, then we Y"), test walkthroughs, proofs of obvious branches. Delete; keep only a non-obvious contract or invariant.
7. **Hedges and planning residue**: "probably fine for now", "should be enough", deferrals with no marker. Turn into a `TODO` with an issue link, or state the actual bound.
8. **Authoring-language slips**: fragments of the working language left in English prose or comments. Translate or delete.

## Probes

Run from the repo root. Exclusions go last so a later include cannot re-admit them; add the repo's fixture and snapshot directories, and this skill's own directory if it is vendored. Every hit needs a human judgement: the probes over-match by design and under-match by nature, so also read the densest prose in scope (module docs, README, `AGENTS.md`) without a pattern in hand.

```sh
X=(--hidden --glob '!.git/**' --glob '!vendor/**' --glob '!third_party/**' --glob '!node_modules/**'
   --glob '!**/fixtures/**' --glob '!**/snapshots/**' --glob '!.agents/skills/agent-docs-workflow/**')
# 1. Session citations and milestone codes (case-sensitive on purpose).
rg -n '\(decision \d|\(audit [A-Z]\d|design §|plan §|\bM\d\b|\bP-?\d\b|\bW\d\b|\bT\d\b' "${X[@]}"
rg --files "${X[@]}" | rg -i '(^|/)(m|p|t|w)\d+[._-]'
# 2. PR and stack vantage.
rg -n -i '\bthis PR\b|\bthis branch\b|\bthis stack\b|\blater PRs?\b|\bprevious commits?\b|\bthis commit\b' "${X[@]}"
# 3. Change narration and stamps.
rg -n -i '\bused to\b|\bno longer\b|\bpreviously\b|\bthe old\b|\bwas renamed\b|\bwas moved\b|\bnow (uses|supports|handles)\b' "${X[@]}"
rg -n -i '\bv1\b|this cut|\btoday\b|\bfor now\b|\bas of v\d' "${X[@]}"
# 4-5. Review choreography and reviewer-addressed prose.
rg -n -i 'rejected in review|review round|\breviewer\b|\bround \d\b' "${X[@]}"
rg -n -i 'this is (correct|safe) because|it simply|is safe —|is safe --' "${X[@]}"
# 7. Hedges.
rg -n -i '\bprobably\b|should be enough|should suffice|good enough for now' "${X[@]}"
# 8. Non-English script in English files.
rg -n '\p{Han}' --glob '*.md' --glob '*.py' --glob '*.rs' --glob '*.ts' "${X[@]}"
```

Before trusting a zero-hit probe, make it match a known positive; before trusting a noisy one, make it reject a near-miss negative. `\bthis PR\b` must match "This PR adds" and not "this project".

## Known false positives

Judge these and keep them:

- **Instrumental "used to"**: "the key used to sign requests". The temporal form has a past state before it ("colors used to come from…").
- **Runtime old/new**: "the old connection drains before the new one accepts" describes live objects, not repo history.
- **"PR" in process docs**: docs about the PR workflow (templates, `workflow.md`, review skills) legitimately say "PR". The ban is on one PR's vantage about the code.
- **`v1` as an identifier**: `/v1/chat`, wire-format and schema versions.
- **`§N` with a committed owner**: external standards ("RFC 9110 §10.1.5") and committed docs that own their numbering.
- **Issue and PR references in their homes**: `#123` and `TODO(#123)` resolve at HEAD. Merged-PR links are evidence in notes, decisions, and plans.
- **Counterfactual regression pins**: "without X, Y happens", "a naive X would…".
- **Measured bounds**: "(measured: 512 nests ≈ 0.15s on <machine>, <date>)" calibrating a constant.
- **Completed plans and decision Alternatives**: history and "rejected" are the genre there.
- **Exempted rule codes**: a project's own rule IDs (`ASYNC101`) or priorities defined in a committed doc are identifiers, not session codes.
- **Recorded output**: fixtures, snapshots, and quoted CLI output keep their voice.

## Fixing

1. Before deleting a passage, list its propositions (see the complete-proposition rule in [SKILL.md](../SKILL.md#writing-rules)). Rewrite so each surviving fact describes current behaviour; delete only the transcript around it. A passage with no factual clause (an audit code, control-flow narration) is deleted outright.
2. Watch for overcorrection: a trim must not turn an obligation into a suggestion, promote a hypothetical to a shipped feature, delete a true fact, or drop the evidence for a number.
3. Renaming a file or test changes its callers; run the tests.
4. Re-run the probes. Remaining hits should be judged keeps only.
