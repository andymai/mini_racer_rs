#!/usr/bin/env bash
# PreToolUse (Bash): refuses commands that skip the git hooks. Exit 2 blocks the
# call and shows the message to the agent. This is a tripwire against an agent
# skipping hooks by habit, not a security boundary: CI reruns every check.
set -euo pipefail

command=$(jq -r '.tool_input.command // empty')
[ -n "$command" ] || exit 0
# Quotes and backslashes cannot hide a flag from this check (--no-ver"ify").
plain=$(tr -d "\"'\\\\" <<<"$command")

ws='[[:space:]]'
token='[^[:space:]]+'
patterns=(
  # git accepts unique prefixes of long options; --no-veri is the shortest
  # that cannot mean --no-verbose.
  '--no-veri'
  "(^|[;&|(]|$ws)HUSKY=0"
  'core\.hooksPath'
  "git($ws+-c$ws+$token)*$ws+commit($ws+$token)*$ws+-[a-zA-Z]*n[a-zA-Z]*($ws|$)"
)
for pattern in "${patterns[@]}"; do
  if grep -qE -- "$pattern" <<<"$plain"; then
    echo "Blocked: this command skips the git hooks. Fix what the hook reports instead; see .claude/skills/pr-workflow/SKILL.md." >&2
    exit 2
  fi
done
exit 0
