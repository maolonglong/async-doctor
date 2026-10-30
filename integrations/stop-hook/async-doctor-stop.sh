#!/usr/bin/env bash
# Shared stop-hook helper for Claude Code and Cursor.
#
# Claude Code: exit 2 + reason on stderr (or JSON decision) to block stop.
#              Infra failures must NOT block (exit 0 + note).
#              Honor stop_hook_active to avoid infinite loops.
# Cursor:      print {"followup_message": "..."} on stdout to continue;
#              honor loop_count; infra failures exit 0 with a note.
#
# Requires `async-doctor` on PATH. No uvx / PyPI fallback.
set -euo pipefail

INPUT="$(cat || true)"
AGENT="${ASYNC_DOCTOR_HOOK_AGENT:-auto}"

detect_agent() {
  if [[ "$AGENT" != "auto" ]]; then
    echo "$AGENT"
    return
  fi
  if echo "$INPUT" | grep -q '"stop_hook_active"'; then
    echo claude
  elif echo "$INPUT" | grep -q '"loop_count"'; then
    echo cursor
  elif echo "$INPUT" | grep -q '"hook_event_name"[[:space:]]*:[[:space:]]*"stop"'; then
    echo cursor
  else
    # Default to Claude-style blocking when invoked without stdin JSON.
    echo claude
  fi
}

AGENT="$(detect_agent)"

# --- loop guards ---
if [[ "$AGENT" == "claude" ]]; then
  if echo "$INPUT" | grep -q '"stop_hook_active"[[:space:]]*:[[:space:]]*true'; then
    exit 0
  fi
fi
if [[ "$AGENT" == "cursor" ]]; then
  # Default Cursor loop_limit is 5; stop asking after a few follow-ups.
  LOOP="$(echo "$INPUT" | sed -n 's/.*"loop_count"[[:space:]]*:[[:space:]]*\([0-9][0-9]*\).*/\1/p' | head -1)"
  if [[ -n "${LOOP:-}" && "$LOOP" -ge 3 ]]; then
    echo '{}'
    exit 0
  fi
  STATUS="$(echo "$INPUT" | sed -n 's/.*"status"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' | head -1)"
  if [[ "${STATUS:-}" == "aborted" || "${STATUS:-}" == "error" ]]; then
    echo '{}'
    exit 0
  fi
fi

ROOT="$(git rev-parse --show-toplevel 2>/dev/null || true)"
if [[ -z "$ROOT" ]]; then
  echo "note: async-doctor stop hook: not a git repo; allowing stop" >&2
  if [[ "$AGENT" == "cursor" ]]; then echo '{}'; fi
  exit 0
fi
cd "$ROOT"

if ! command -v async-doctor >/dev/null 2>&1; then
  echo "note: async-doctor not found on PATH; install it (no uvx fallback). Allowing stop." >&2
  if [[ "$AGENT" == "cursor" ]]; then echo '{}'; fi
  exit 0
fi

set +e
OUT="$(async-doctor --changed --expand 2>&1)"
CODE=$?
set -e

if [[ "$CODE" -eq 0 ]]; then
  if [[ "$AGENT" == "cursor" ]]; then echo '{}'; fi
  exit 0
fi

if [[ "$CODE" -eq 2 ]]; then
  echo "note: async-doctor tool error (exit 2); allowing stop. Output:" >&2
  echo "$OUT" >&2
  if [[ "$AGENT" == "cursor" ]]; then echo '{}'; fi
  exit 0
fi

# Findings (exit 1) — block / follow up.
REASON="async-doctor found blocking IO on changed files. Fix the findings, then finish.

$OUT"

json_string() {
  # Emit a JSON string literal for $1 (no surrounding object).
  local esc
  esc=$(printf '%s' "$1" | sed 's/\\/\\\\/g; s/"/\\"/g' | awk '{printf "%s\\n", $0}' | sed '$ s/\\n$//')
  printf '"%s"' "$esc"
}

if [[ "$AGENT" == "cursor" ]]; then
  if command -v python3 >/dev/null 2>&1; then
    python3 -c 'import json,sys; print(json.dumps({"followup_message": sys.argv[1]}))' "$REASON"
  else
    printf '{"followup_message":%s}\n' "$(json_string "$REASON")"
  fi
  exit 0
fi

# Claude Code: exit 2 blocks Stop; stderr is the continuation instruction.
# Also emit JSON decision for clients that prefer it.
if command -v python3 >/dev/null 2>&1; then
  python3 -c 'import json,sys; print(json.dumps({"decision": "block", "reason": sys.argv[1]}))' "$REASON"
else
  printf '{"decision":"block","reason":%s}\n' "$(json_string "$REASON")"
fi
echo "$REASON" >&2
exit 2
