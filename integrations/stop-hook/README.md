# Stop hook: block completion on new blocking-IO findings

Generic examples that run `async-doctor --changed` when a coding agent finishes
a turn. Requires `async-doctor` on `PATH` (no `uvx` fallback — that name is not
a registered package).

[`async-doctor-stop.sh`](async-doctor-stop.sh) auto-detects Claude Code vs
Cursor from the stdin JSON payload (or set `ASYNC_DOCTOR_HOOK_AGENT=claude|cursor`).

## Behaviour

| Situation | Claude Code | Cursor |
|-----------|-------------|--------|
| Clean (`async-doctor` exit 0) | exit 0 — allow stop | `{}` on stdout |
| Findings (exit 1) | **exit 2** + reason on stderr (and `{"decision":"block","reason":…}` on stdout) — continues the turn | `{"followup_message":…}` on stdout — auto follow-up |
| Tool / infra failure (missing binary, not a git repo, exit 2) | **exit 0** + note on stderr — do not block | `{}` + note |
| `stop_hook_active: true` / high `loop_count` | exit 0 — avoid loops | `{}` — avoid loops |

Claude Code: only exit **2** blocks Stop (exit 1 is a non-blocking hook error).
Cursor: stop hooks continue via `followup_message`, not exit codes.

## Script

```bash
chmod +x integrations/stop-hook/async-doctor-stop.sh
```

## Claude Code

`.claude/settings.json` (or user settings):

```json
{
  "hooks": {
    "Stop": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "./integrations/stop-hook/async-doctor-stop.sh",
            "timeout": 120
          }
        ]
      }
    ]
  }
}
```

## Cursor

`.cursor/hooks.json` (see [Cursor hooks docs](https://cursor.com/docs/hooks)):

```json
{
  "version": 1,
  "hooks": {
    "stop": [
      {
        "command": "./integrations/stop-hook/async-doctor-stop.sh",
        "loop_limit": 5
      }
    ]
  }
}
```

On findings the script prints `{"followup_message":"..."}` on stdout (exit 0). Cursor
auto-submits that as the next user message. Honor `loop_count` / `status` from the
stdin payload so aborted/errored stops and high loop counts do not keep looping.

## Tips

- Prefer `--changed` so the agent loop stays fast; analysis is still whole-project.
- Pair with the [skill](../skill/SKILL.md) so the agent knows how to fix findings.
- For backlog suppression in CI only: `--baseline baseline.json`.
- Claude Code: see [hooks reference](https://code.claude.com/docs/en/hooks) — Stop
  blocks only on exit **2** (or JSON `decision: "block"`); always honor
  `stop_hook_active`.
