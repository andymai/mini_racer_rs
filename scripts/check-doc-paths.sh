#!/usr/bin/env bash
# Agents load CLAUDE.md and the skills as ground truth, and readers follow the
# public docs, so every repository path they name in backticks must exist. A
# renamed or split file is the usual drift. Upstream test paths are written
# relative to the suite's directory (vendor/mini_racer or vendor/execjs), which
# test/upstream/run.rb resolves them against, and a bare workflow file name
# resolves under .github/workflows, so they may resolve there instead.
set -euo pipefail
cd "$(dirname "$0")/.."

# Strings that look like paths but name none: build outputs the docs describe
# on purpose, the repository's GitHub slug, and slash-joined names in prose. An
# entry ending in / covers everything under it.
allowed=(
  "andymai/mini_racer_rs"
  "call_await/eval_await"
  "class/object"
  "lib/mini_racer_rs/mini_racer_rs.so"
  "pkg/"
  "target/"
  "tmp/"
)

# Candidate paths come from every backtick span and fenced code block, split into
# words, so a path inside a command (`scripts/gate.sh --full`) is checked too.
fenced_blocks() {
  awk '/^```/ { fenced = !fenced; next } fenced { print }' "$1"
}

# The verbatim third-party texts are left as written.
docs=()
for doc in *.md .claude/skills/*/*.md .github/*.md .github/ISSUE_TEMPLATE/*.md; do
  case "$doc" in
    CODE_OF_CONDUCT.md | THIRD_PARTY_NOTICES.md | AI-DISCLOSURE.md) ;;
    *) [ -f "$doc" ] && docs+=("$doc") ;;
  esac
done

fail=0
for doc in "${docs[@]}"; do
  while IFS= read -r path; do
    [ -n "$path" ] || continue
    for skip in "${allowed[@]}"; do
      case "$skip" in
        */) [[ "$path" == "$skip"* ]] && continue 2 ;;
        *) [ "$path" = "$skip" ] && continue 2 ;;
      esac
    done
    if [ ! -e "$path" ] && [ ! -e "vendor/mini_racer/$path" ] && [ ! -e "vendor/execjs/$path" ] &&
      [ ! -e ".github/workflows/$path" ]; then
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
echo "Paths named in the docs, CLAUDE.md and the skills exist."
