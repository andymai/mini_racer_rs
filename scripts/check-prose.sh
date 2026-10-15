#!/usr/bin/env bash
# No en dashes (U+2013) or em dashes (U+2014) in anything this project writes.
# Usage: check-prose.sh [--vault <path> | --commit-msg <file>]
set -euo pipefail

dashes=$'\xe2\x80[\x93\x94]'

scan() {
  LC_ALL=C grep -nE "$dashes" "$@" || true
}

case "${1:-}" in
  --commit-msg)
    hits=$(grep -v '^#' "$2" | LC_ALL=C grep -nE "$dashes" || true)
    ;;
  --vault)
    vault="$2"
    hits=$(
      scan "$vault"/Projects/mini_racer_rs/*.md
      grep -n 'mini_racer_rs' "$vault/index.md" | LC_ALL=C grep -E "$dashes" | sed 's/^/index.md:/' || true
    )
    ;;
  "")
    cd "$(dirname "$0")/.."
    if git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
      list() { git ls-files -co --exclude-standard; }
    else
      list() { find . -type f -not -path './.git/*' -not -path './target/*' -not -path './tmp/*' -not -path './node_modules/*' | sed 's|^\./||'; }
    fi
    hits=$(list | grep -vE \
      '^(LICENSE|licenses/|THIRD_PARTY_NOTICES\.md|CODE_OF_CONDUCT\.md|vendor/|lib/mini_racer/(shared|truffleruby)\.rb|Cargo\.lock|Gemfile\.lock|package-lock\.json)' |
      while IFS= read -r f; do [ -f "$f" ] && printf '%s\0' "$f"; done |
      xargs -0 sh -c 'LC_ALL=C grep -nE "$0" "$@" /dev/null || true' "$dashes")
    ;;
  *)
    echo "usage: $0 [--vault <path> | --commit-msg <file>]" >&2
    exit 2
    ;;
esac

if [ -n "$hits" ]; then
  sed 's/^/VIOLATION [dash]: /' <<<"$hits"
  echo "Use a period, comma, colon, or parentheses instead of an en or em dash." >&2
  exit 1
fi
echo "No en or em dashes."
