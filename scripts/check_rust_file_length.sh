#!/usr/bin/env bash
# Fail if any .rs file under crates/ exceeds 800 lines (AGENTS.md hard ceiling).
# Fixtures and generated files are outside crates/ and are exempt.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
MAX=800
failed=0

while IFS= read -r -d '' file; do
  lines=$(wc -l < "$file")
  if (( lines > MAX )); then
    printf 'error: %s has %d lines (limit %d)\n' "${file#"$ROOT"/}" "$lines" "$MAX" >&2
    failed=1
  fi
done < <(find "$ROOT/crates" -type f -name '*.rs' -print0 | sort -z)

if (( failed )); then
  exit 1
fi

echo "OK: all crates/**/*.rs files are <= ${MAX} lines"
