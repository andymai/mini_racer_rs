#!/usr/bin/env bash
# PostToolUse (Edit, Write): reruns the layer check after any Cargo.toml or
# layers.toml edit. Exit 2 shows the violations to the agent.
set -euo pipefail

file=$(jq -r '.tool_input.file_path // empty')
case "$file" in
  */Cargo.toml | */layers.toml) ;;
  *) exit 0 ;;
esac

root=$(git -C "$(dirname "$file")" rev-parse --show-toplevel 2>/dev/null) || exit 0
[ -x "$root/scripts/check-boundaries.sh" ] || exit 0
if ! output=$("$root/scripts/check-boundaries.sh" 2>&1); then
  echo "$output" >&2
  exit 2
fi
exit 0
