#!/usr/bin/env bash
# Block agent completion when async-doctor reports new findings on changed files.
# Exit codes mirror async-doctor: 0 clean, 1 findings, 2 tool error.
set -euo pipefail

ROOT="$(git rev-parse --show-toplevel 2>/dev/null || pwd)"
cd "$ROOT"

if command -v async-doctor >/dev/null 2>&1; then
  exec async-doctor --changed
elif command -v uvx >/dev/null 2>&1; then
  exec uvx async-doctor --changed
else
  echo "async-doctor not found on PATH (and uvx unavailable)" >&2
  exit 2
fi
