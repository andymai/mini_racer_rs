#!/usr/bin/env python3
# PostToolUse (Edit, Write): reruns the layer check after a Cargo.toml or
# layers.toml edit in this project. Exit 2 shows the violations to the agent.
# It runs only this project's own script, never one found next to the edited
# file, so editing a manifest in another checkout executes nothing from it.
import json
import os
import subprocess
import sys
from pathlib import Path


def main():
    project = os.environ.get("CLAUDE_PROJECT_DIR")
    if not project:
        return 0
    project = Path(project).resolve()
    path = json.load(sys.stdin).get("tool_input", {}).get("file_path") or ""
    if Path(path).name not in ("Cargo.toml", "layers.toml"):
        return 0
    if not Path(path).resolve().is_relative_to(project):
        return 0
    result = subprocess.run(
        ["bash", str(project / "scripts" / "check-boundaries.sh")],
        cwd=project,
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        print(result.stdout + result.stderr, file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    sys.exit(main())
