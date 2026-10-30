# Stop hook: block completion on new blocking-IO findings

Generic examples that run `async-doctor --changed` when a coding agent finishes
a turn. Exit code `1` means findings remain — the agent should keep fixing.
Exit `0` allows completion. Exit `2` is a tool error (treat as a failure).

No company-specific paths. Requires `async-doctor` on `PATH` (or adjust the
command to `uvx async-doctor` / `cargo run -p async_doctor --`).

## Script

[`async-doctor-stop.sh`](async-doctor-stop.sh) runs from the project root and
exits with async-doctor's status.

```bash
chmod +x integrations/stop-hook/async-doctor-stop.sh
./integrations/stop-hook/async-doctor-stop.sh
```

## Claude Code

Add a Stop hook in `.claude/settings.json` (or user settings):

```json
{
  "hooks": {
    "Stop": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "async-doctor --changed",
            "timeout": 120
          }
        ]
      }
    ]
  }
}
```

Or point `command` at this repo's script:

```json
{
  "type": "command",
  "command": "./integrations/stop-hook/async-doctor-stop.sh"
}
```

## Cursor

Use a project hook that runs after the agent stops editing. Example
`.cursor/hooks.json` fragment:

```json
{
  "version": 1,
  "hooks": {
    "stop": [
      {
        "command": "async-doctor --changed"
      }
    ]
  }
}
```

Exact Cursor hook schema may evolve — keep the command identical; only the
wrapper JSON changes.

## Tips

- Prefer `--changed` so the agent loop stays fast; analysis is still whole-project.
- Pair with the [skill](../skill/SKILL.md) so the agent knows how to fix findings.
- For backlog suppression in CI only: `--baseline baseline.json`.
