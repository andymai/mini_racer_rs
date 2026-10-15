#!/usr/bin/env bash
# Feeds sample tool calls to the Claude Code hooks and checks which are blocked.
set -euo pipefail
cd "$(dirname "$0")/.."

fail=0
run() {
  local expected="$1" command="$2" status=0
  jq -n --arg c "$command" '{tool_input: {command: $c}}' | .claude/hooks/block-bypass.sh >/dev/null 2>&1 || status=$?
  if [ "$status" -ne "$expected" ]; then
    echo "FAIL: exit $status, expected $expected: $command"
    fail=1
  fi
}

run 2 'git commit --no-verify -m "fix: x"'
run 2 'git commit -n -m "fix: x"'
run 2 'git commit -anm "fix: x"'
run 2 'git push --no-verify origin main'
run 2 'HUSKY=0 git commit -m "fix: x"'
run 2 'cd repo && HUSKY=0 git commit -m "fix: x"'
run 2 'git -c core.hooksPath=/dev/null commit -m "fix: x"'
run 0 'git commit -m "fix: x"'
run 0 'git commit --amend --no-edit'
run 0 'git status'
run 0 'cargo nextest run --workspace'

status=0
jq -n '{tool_input: {file_path: "/tmp/notes.md"}}' | .claude/hooks/check-boundaries-on-manifest.sh >/dev/null 2>&1 || status=$?
if [ "$status" -ne 0 ]; then
  echo "FAIL: the manifest hook acted on a non-manifest file"
  fail=1
fi

if [ "$fail" -ne 0 ]; then
  exit 1
fi
echo "Claude hooks block exactly the hook-skipping commands."
