#!/usr/bin/env bash
# Asserts the GitHub repository settings the project depends on. Needs an admin
# token, so it runs locally (gh auth), not in CI.
set -euo pipefail

repo="${REPO:-andymai/mini_racer_rs}"
fail=0
expect() {
  local what="$1" actual="$2" wanted="$3"
  if [ "$actual" != "$wanted" ]; then
    echo "VIOLATION [settings]: $what is $actual, expected $wanted"
    fail=1
  fi
}

settings=$(gh api "repos/$repo")
for pair in visibility=public allow_squash_merge=true allow_merge_commit=false allow_rebase_merge=false \
  allow_auto_merge=true delete_branch_on_merge=true squash_merge_commit_title=PR_TITLE \
  squash_merge_commit_message=PR_BODY; do
  expect "${pair%%=*}" "$(jq -r ".${pair%%=*}" <<<"$settings")" "${pair#*=}"
done
expect "secret scanning" "$(jq -r '.security_and_analysis.secret_scanning.status' <<<"$settings")" enabled
expect "push protection" "$(jq -r '.security_and_analysis.secret_scanning_push_protection.status' <<<"$settings")" enabled
expect "private vulnerability reporting" "$(gh api "repos/$repo/private-vulnerability-reporting" --jq .enabled)" true
expect "actions SHA pinning" "$(gh api "repos/$repo/actions/permissions" --jq .sha_pinning_required)" true
expect "RELEASE_HOLD" "$(gh variable get RELEASE_HOLD -R "$repo")" true
expect "Dependabot alerts" "$(gh api "repos/$repo/vulnerability-alerts" --silent 2>/dev/null && echo enabled || echo disabled)" enabled
expect "Dependabot security updates" "$(gh api "repos/$repo/automated-security-fixes" --jq .enabled)" true

codeql=$(gh api "repos/$repo/code-scanning/default-setup")
expect "CodeQL default setup" "$(jq -r .state <<<"$codeql")" configured
for language in actions ruby; do
  expect "CodeQL covers $language" "$(jq -r --arg l "$language" '.languages | index($l) != null' <<<"$codeql")" true
done

env=$(gh api "repos/$repo/environments/rubygems")
expect "rubygems reviewers" "$(jq -r '[.protection_rules[] | select(.type == "required_reviewers") | .reviewers[].reviewer.login] | join(",")' <<<"$env")" andymai
expect "rubygems deployment policy" "$(gh api "repos/$repo/environments/rubygems/deployment-branch-policies" --jq '[.branch_policies[] | "\(.type):\(.name)"] | join(",")')" "tag:v*"

ruleset_id=$(gh api "repos/$repo/rulesets" --jq '.[] | select(.name == "main") | .id')
ruleset=$(gh api "repos/$repo/rulesets/$ruleset_id")
expect "ruleset enforcement" "$(jq -r .enforcement <<<"$ruleset")" active
expect "ruleset target" "$(jq -r '.conditions.ref_name.include | join(",")' <<<"$ruleset")" "~DEFAULT_BRANCH"
expect "ruleset rules" "$(jq -r '[.rules[].type] | sort | join(",")' <<<"$ruleset")" \
  "deletion,non_fast_forward,pull_request,required_linear_history,required_status_checks"
pr=$(jq '.rules[] | select(.type == "pull_request") | .parameters' <<<"$ruleset")
expect "required approvals" "$(jq -r .required_approving_review_count <<<"$pr")" 0
expect "thread resolution" "$(jq -r .required_review_thread_resolution <<<"$pr")" true
expect "merge methods" "$(jq -r '.allowed_merge_methods | join(",")' <<<"$pr")" squash
checks=$(jq -r '.rules[] | select(.type == "required_status_checks") | .parameters.required_status_checks | map("\(.context)@\(.integration_id)") | sort | join(",")' <<<"$ruleset")
expect "required checks" "$checks" "CI Pass@15368,Greptile Review@867647"

if [ "$fail" -ne 0 ]; then
  exit 1
fi
echo "Repository settings match."
