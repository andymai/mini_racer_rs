#!/usr/bin/env bash
# A blocked ordinary command teaches an agent to route around the hook, so the
# look-alike cases matter as much as the bypasses.
set -euo pipefail
cd "$(dirname "$0")/.."

fail=0
run() {
  local expected="$1" command="$2" status=0
  jq -n --arg c "$command" '{tool_input: {command: $c}}' | .claude/hooks/block-bypass.py >/dev/null 2>&1 || status=$?
  if [ "$status" -ne "$expected" ]; then
    echo "FAIL: exit $status, expected $expected: $command"
    fail=1
  fi
}

run 2 'git commit --no-verify -m "fix: x"'
run 2 'git commit --no-veri -m "fix: x"'
run 2 'git commit --no-ver"ify" -m "fix: x"'
run 2 'git commit -n -m "fix: x"'
run 2 'git commit -anm "fix: x"'
run 2 'git commit -m "fix: x" -n'
run 2 'git -C .worktrees/feat commit -n -m "fix: x"'
run 2 'git --no-pager commit -n -m "fix: x"'
run 2 'git push --no-verify origin main'
run 2 'git push --no-veri origin main'
run 2 'HUSKY=0 git commit -m "fix: x"'
run 2 'HUSKY="0" git commit -m "fix: x"'
run 2 'cd repo && HUSKY=0 git commit -m "fix: x"'
run 2 'export HUSKY=0'
run 2 'bash -c "HUSKY=0 git commit -m x"'
run 2 'git -c core.hooksPath=/dev/null commit -m "fix: x"'
run 2 'git -c core.hookspath=/dev/null commit -m "fix: x"'
run 2 'git config core.hookspath /dev/null'
run 2 "GIT_CONFIG_PARAMETERS=\"'core.hookspath'='/dev/null'\" git commit -m x"
run 2 $'git status\ngit commit -n -m x'
run 2 'git commit -m ";" -n'
run 2 'git -C "$PWD"/.worktrees/feat commit -n -m x'
run 2 $'git commit -m "fix: x" \\\n  -n'
run 2 'echo "$(git commit -n -m x)"'
run 2 'git commit -n -m "fix: x" # a comment'

run 0 'git commit -m "fix: x"'
run 0 'git commit --amend --no-edit'
run 0 'git commit --no-verbose -m "fix: x"'
run 0 'git commit -m "fix: x" 2>&1 | tail -n 20'
run 0 'git commit -m "fix: x" && git log --oneline -n 3'
run 0 'git commit -m x; ls -n'
run 0 'git commit -m "feat: add -n flag"'
run 0 'git commit -m "docs: explain --no-verify"'
run 0 'git commit -uno -m "fix: x"'
run 0 'git config --get core.hooksPath'
run 0 'rg -n core.hooksPath .'
run 0 'git status'
run 0 'cargo nextest run --workspace'
run 0 $'git commit -F - <<\'MSG\'\nfix: x\n\nbody -n --no-verify\nMSG'
run 0 $'git commit -m "fix: x\n\nsecond line"'
run 0 "git commit -m 'docs: explain \$(git commit -n -m x)'"
run 0 "git commit -m 'docs: update guidance' # never use --no-verify"

manifest() {
  local expected="$1" project="$2" file="$3" status=0
  jq -n --arg f "$file" '{tool_input: {file_path: $f}}' |
    CLAUDE_PROJECT_DIR="$project" .claude/hooks/check-boundaries-on-manifest.py >/dev/null 2>&1 || status=$?
  if [ "$status" -ne "$expected" ]; then
    echo "FAIL: manifest hook exit $status, expected $expected: $file"
    fail=1
  fi
}

stub() {
  mkdir -p "$1/scripts"
  printf '#!/bin/sh\nexit %s\n' "$2" >"$1/scripts/check-boundaries.sh"
  touch "$1/Cargo.toml" "$1/README.md"
}

scratch=$(mktemp -d)
stub "$scratch/failing" 1
stub "$scratch/passing" 0
stub "$scratch/elsewhere" 1
manifest 2 "$scratch/failing" "$scratch/failing/Cargo.toml"
manifest 0 "$scratch/failing" "$scratch/failing/README.md"
manifest 0 "$scratch/passing" "$scratch/passing/Cargo.toml"
manifest 0 "$scratch/passing" "$scratch/elsewhere/Cargo.toml"
rm -rf "$scratch"

if [ "$fail" -ne 0 ]; then
  exit 1
fi
echo "Claude hooks block the hook-skipping commands and nothing else sampled."
