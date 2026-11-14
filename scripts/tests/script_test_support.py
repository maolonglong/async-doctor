"""Standard-library fixtures for exercising the public audit/bench scripts."""

import json
import os
import subprocess
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]


def report(**updates):
    data = {
        "schemaVersion": 2,
        "complete": True,
        "score": 100,
        "environment": {
            "kind": "system",
            "origin": "system",
            "sitePackages": [],
            "resolvedThirdParty": 0,
            "unresolvedThirdParty": 0,
        },
        "findings": [],
        "candidates": [],
        "advice": [],
        "locationLessCandidates": [],
        "skipped": [],
        "sinkSites": [],
        "unusedSuppressions": [],
        "unresolvedSummary": [],
        "hiddenUnresolvedCandidates": 0,
        "omittedNonProductionEntries": 0,
        "fileCount": 1,
        "asyncEntryCount": 1,
        "omittedFindings": 0,
        "omittedCandidates": 0,
        "omittedAdvice": 0,
    }
    data.update(updates)
    return data


def diagnostic(identifier="id-1", line=1):
    return {
        "id": identifier,
        "rule": "blocking-sleep",
        "entry": {"file": "app.py", "line": line, "name": "main", "label": "async_def"},
        "chain": [{"file": "app.py", "line": line + 1, "text": "time.sleep(1)"}],
        "conditions": [],
        "fix": {"hint": "await asyncio.sleep(1)", "fixGroupId": "group"},
        "why": "async-doctor why app.py:1",
    }


def executable(path, source):
    path.write_text(f"#!{sys.executable}\n" + source, encoding="utf-8")
    path.chmod(0o755)
    return path


def scanner(path, stdout, exit_code=0, after_output=""):
    return executable(
        path,
        "import sys\n"
        f"sys.stdout.write({stdout!r})\n"
        "sys.stdout.flush()\n"
        f"{after_output}\n"
        f"raise SystemExit({exit_code})\n",
    )


def write_report(path, data=None):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(report() if data is None else data), encoding="utf-8")
    return path


def run_script(*args, env=None):
    child_env = os.environ.copy()
    child_env.pop("BENCH_STRICT", None)
    if env:
        child_env.update(env)
    return subprocess.run(
        [str(arg) for arg in args],
        cwd=ROOT,
        env=child_env,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        timeout=15,
        check=False,
    )
