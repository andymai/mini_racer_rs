#!/usr/bin/env bash
# Reports each release gem's size, and fails if one is over the budget in
# release/size-budget.toml. Without a budget the sizes are only recorded. No
# budget may pass 75 MB, the size that reopens the packaging plan; without
# one, a gem over it is a warning.
# Usage: check-gem-sizes.sh <dir with .gem files>
set -euo pipefail
cd "$(dirname "$0")/.."

dir=${1:?usage: check-gem-sizes.sh <dir with .gem files>}
ceiling=75000000
budget=
if [ -f release/size-budget.toml ]; then
  budget=$(taplo get -f release/size-budget.toml max_bytes)
  [ "$budget" -le "$ceiling" ] || { echo "VIOLATION [size]: budget $budget is over the $ceiling ceiling"; exit 1; }
fi
summary=${GITHUB_STEP_SUMMARY:-/dev/null}
printf '| Gem | Bytes |\n|---|---|\n' >>"$summary"
fail=0
for gem in "$dir"/*.gem; do
  bytes=$(wc -c <"$gem" | tr -d ' ')
  name=$(basename "$gem")
  echo "$name: $bytes bytes"
  echo "| $name | $bytes |" >>"$summary"
  if [ -n "$budget" ] && [ "$bytes" -gt "$budget" ]; then
    echo "VIOLATION [size]: $name is over the $budget-byte budget"
    fail=1
  elif [ "$bytes" -gt "$ceiling" ]; then
    echo "::warning::$name is over $ceiling bytes, which reopens the packaging plan"
  fi
done
[ -n "$budget" ] || echo "No release/size-budget.toml yet: sizes recorded only."
exit "$fail"
