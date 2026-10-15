#!/usr/bin/env bash
# Agents load CLAUDE.md and the skills as ground truth, so every repository path
# they name in backticks must exist. A renamed or split file is the usual drift.
set -euo pipefail
cd "$(dirname "$0")/.."

# Build outputs the docs describe on purpose.
allowed=(
  "lib/mini_racer_rs/mini_racer_rs.so"
  "target/"
  "tmp/"
)

# Candidate paths come from every backtick span and fenced code block, split into
# words, so a path inside a command (`scripts/gate.sh --full`) is checked too.
fenced_blocks() {
  awk '/^```/ { fenced = !fenced; next } fenced { print }' "$1"
}

fail=0
for doc in CLAUDE.md .claude/skills/*/*.md; do
  [ -f "$doc" ] || continue
  while IFS= read -r path; do
    [ -n "$path" ] || continue
    for skip in "${allowed[@]}"; do
      [ "$path" = "$skip" ] && continue 2
    done
    if [ ! -e "$path" ]; then
      echo "VIOLATION [doc-path]: $doc names $path, which does not exist"
      fail=1
    fi
  done < <({ grep -oE '`[^`]+`' "$doc" | tr -d '`'; fenced_blocks "$doc"; } |
    tr -s ' \t()"' '\n' |
    grep -E '^\.?[A-Za-z0-9_][A-Za-z0-9_.-]*(/[A-Za-z0-9_.-]+)*/?$' |
    grep -E '/|\.(rs|rb|sh|py|toml|ya?ml|json|md|lock|gemspec|sha256)$' |
    grep -vE '::|^https?|\.\.' | sort -u || true)
done

if [ "$fail" -ne 0 ]; then
  exit 1
fi
echo "Paths named in CLAUDE.md and the skills exist."
