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
run 2 'git commit --no-ver"ify" -m "fix: x"'
run 2 'git commit --no-verif\\y -m "fix: x"'
run 2 'git commit --no-veri -m "fix: x"'
run 0 'git commit -m "fix: x"'
run 0 'git commit --amend --no-edit'
run 0 'git commit --no-verbose -m "fix: x"'
run 0 'git status'
run 0 'cargo nextest run --workspace'

manifest() {
  local expected="$1" file="$2" status=0
  jq -n --arg f "$file" '{tool_input: {file_path: $f}}' |
    CLAUDE_PROJECT_DIR="$PWD" .claude/hooks/check-boundaries-on-manifest.sh >/dev/null 2>&1 || status=$?
  if [ "$status" -ne "$expected" ]; then
    echo "FAIL: manifest hook exit $status, expected $expected: $file"
    fail=1
  fi
}

# A manifest outside the project must not run anything, even a failing script.
outside=$(mktemp -d)
mkdir -p "$outside/scripts"
printf '#!/bin/sh\nexit 1\n' >"$outside/scripts/check-boundaries.sh"
chmod +x "$outside/scripts/check-boundaries.sh"
git -C "$outside" init -q
touch "$outside/Cargo.toml"
manifest 0 "$outside/Cargo.toml"
rm -rf "$outside"
manifest 0 "$PWD/README.md"
manifest 0 "$PWD/layers.toml"

if [ "$fail" -ne 0 ]; then
  exit 1
fi
echo "Claude hooks block exactly the hook-skipping commands."
