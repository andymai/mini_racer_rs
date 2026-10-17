#!/usr/bin/env bash
# scheduled-failures.yml watches workflows by name, so a scheduled workflow that
# is renamed or added would drop out of the watch without a word. Every
# workflow with a schedule must be listed there, and every listed name must
# belong to one.
set -euo pipefail
cd "$(dirname "$0")/.."

unquote() { sed -E 's/^ +//; s/ +$//; s/^"(.*)"$/\1/; s/^'"'"'(.*)'"'"'$/\1/'; }

watch=.github/workflows/scheduled-failures.yml
listed=$(sed -nE 's/^ *workflows: \[(.*)\]$/\1/p' "$watch" | tr ',' '\n' | unquote | sort)
[ -n "$listed" ] || { echo "VIOLATION [watch]: $watch lists no workflows"; exit 1; }

fail=0
shopt -s nullglob
scheduled=""
for file in .github/workflows/*.yml .github/workflows/*.yaml; do
  grep -qE '(^[[:space:]]+|[{,][[:space:]]*)schedule:' "$file" || continue
  name=$(sed -nE 's/^name:(.*)$/\1/p' "$file" | head -n 1 | unquote)
  if [ -z "$name" ]; then
    echo "VIOLATION [watch]: the scheduled workflow $file has no name: for $watch to match"
    fail=1
  else
    scheduled+="$name"$'\n'
  fi
done
scheduled=$(printf '%s' "$scheduled" | sort)

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
