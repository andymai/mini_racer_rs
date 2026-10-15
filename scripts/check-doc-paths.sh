#!/usr/bin/env bash
# Agents load CLAUDE.md and the skills as ground truth, so every repository path
# they name in backticks must exist. A renamed or split file is the usual drift.
set -euo pipefail
cd "$(dirname "$0")/.."

# Build outputs the docs describe on purpose.
allowed=(
  "lib/mini_racer_rs/mini_racer_rs.so"
)

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
  done < <(grep -oE '`[A-Za-z0-9_.][A-Za-z0-9_./-]*`' "$doc" | tr -d '`' |
    grep -E '/|\.(rs|rb|sh|toml|ya?ml|json|md|lock|gemspec|sha256)$' |
    grep -vE '^\.\.?$|::' | sort -u || true)
done

if [ "$fail" -ne 0 ]; then
  exit 1
fi
echo "Paths named in CLAUDE.md and the skills exist."
