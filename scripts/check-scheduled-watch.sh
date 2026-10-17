#!/usr/bin/env bash
# scheduled-failures.yml watches workflows by name, so a scheduled workflow that
# is renamed or added would drop out of the watch without a word. Every
# workflow with a schedule must be listed there, and every listed name must
# belong to one.
set -euo pipefail
cd "$(dirname "$0")/.."

watch=.github/workflows/scheduled-failures.yml
listed=$(sed -nE 's/^ *workflows: \[(.*)\]$/\1/p' "$watch" | tr ',' '\n' | sed -E 's/^ +//; s/ +$//' | sort)
[ -n "$listed" ] || { echo "VIOLATION [watch]: $watch lists no workflows"; exit 1; }
scheduled=$(for file in .github/workflows/*.yml; do
  if grep -qE '^  schedule:' "$file"; then sed -nE 's/^name: (.*)$/\1/p' "$file"; fi
done | sort)

fail=0
while IFS= read -r name; do
  echo "VIOLATION [watch]: the scheduled workflow \"$name\" is missing from $watch"
  fail=1
done < <(comm -13 <(echo "$listed") <(echo "$scheduled"))
while IFS= read -r name; do
  echo "VIOLATION [watch]: $watch lists \"$name\", which no scheduled workflow is named"
  fail=1
done < <(comm -23 <(echo "$listed") <(echo "$scheduled"))
[ "$fail" = 0 ] || exit 1
echo "Every scheduled workflow is watched for failures."
