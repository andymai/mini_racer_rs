#!/usr/bin/env bash
# Opens one issue per workflow when its scheduled run fails, or comments on the
# one already open, so a failure that keeps recurring stays in one thread. A
# scheduled run reaches no pull request, so without this nobody hears of it.
#   RUN_ID=<run id> WORKFLOW=<workflow name> scripts/report-scheduled-failure.sh [--dry-run]
set -euo pipefail
: "${RUN_ID:?set RUN_ID}" "${WORKFLOW:?set WORKFLOW}"
dry_run=false
[ "${1:-}" = "--dry-run" ] && dry_run=true

run=$(gh api "repos/{owner}/{repo}/actions/runs/$RUN_ID")
url=$(jq -r .html_url <<<"$run")
sha=$(jq -r .head_sha <<<"$run")
jobs=$(gh api --paginate "repos/{owner}/{repo}/actions/runs/$RUN_ID/jobs" \
  --jq '.jobs[] | select(.conclusion == "failure" or .conclusion == "timed_out") | "- [\(.name)](\(.html_url))"')
title="Scheduled $WORKFLOW run failed"
body=$(printf 'Run: %s\nCommit: %s\n\nFailed jobs:\n%s\n' "$url" "$sha" "${jobs:-- none reported}")
open=$(gh issue list --state open --search "\"$title\" in:title" --json number,title |
  jq -r --arg title "$title" 'map(select(.title == $title)) | .[0].number // empty')

if $dry_run; then
  printf 'issue: %s\ntitle: %s\n%s\n' "${open:-new}" "$title" "$body"
elif [ -n "$open" ]; then
  gh issue comment "$open" --body "$body"
else
  gh issue create --title "$title" --body "$body"
fi
