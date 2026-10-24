#!/usr/bin/env bash
# Runs a command, and once more if it fails. A pass on the second attempt
# passes, but leaves a warning annotation and a line in the job summary naming
# what failed the first time: the upstream suites' ledger mismatches when the
# output holds them, else the exit status. A flake then stays in view however
# often the retry hides it.
# Usage: retry.sh <step name> <command> [args...]
set -euo pipefail

usage="usage: retry.sh <step name> <command> [args...]"
step=${1:?$usage}
shift
[ "$#" -gt 0 ] || { echo "$usage" >&2; exit 2; }
log=$(mktemp)
trap 'rm -f "$log"' EXIT

status=0
"$@" 2>&1 | tee "$log" || status=${PIPESTATUS[0]}
[ "$status" -ne 0 ] || exit 0

# test/upstream/run.rb prints a ledger mismatch as "<file>#<class>#<test>: <verdict>"
# and a suite process that crashed or hung as "suite failure: <message>".
failed=$(awk '
  /^suite failure: / { line = $0 }
  /^[^[:space:]]+#[^[:space:]]+: / { line = $0; sub(/: .*/, "", line) }
  line != "" && !(line in seen) { seen[line] = 1; out = out (out == "" ? "" : "; ") line }
  { line = "" }
  END { print out }
' "$log")
echo "::notice::$step failed (exit $status); running it once more"
"$@"

message="$step passed on retry; the first attempt failed: ${failed:-exit $status}"
echo "::warning title=Passed on retry::$message"
if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then
  echo "- $message" >>"$GITHUB_STEP_SUMMARY"
fi
