#!/usr/bin/env bash
# PostToolUse (Edit, Write): reruns the layer check after a Cargo.toml or
# layers.toml edit in this project. Exit 2 shows the violations to the agent.
# It runs only this project's own script, never one found next to the edited
# file, so editing a manifest in another checkout executes nothing from it.
set -euo pipefail

project="${CLAUDE_PROJECT_DIR:-}"
[ -n "$project" ] || exit 0
project=$(cd "$project" && pwd -P)

file=$(jq -r '.tool_input.file_path // empty')
case "$file" in
  */Cargo.toml | */layers.toml) ;;
  *) exit 0 ;;
esac
dir=$(cd "$(dirname "$file")" 2>/dev/null && pwd -P) || exit 0
case "$dir/" in
  "$project"/*) ;;
  *) exit 0 ;;
esac

if ! output=$("$project/scripts/check-boundaries.sh" 2>&1); then
  echo "$output" >&2
  exit 2
fi
exit 0
