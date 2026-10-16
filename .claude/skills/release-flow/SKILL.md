---
name: release-flow
description: Use when cutting, holding, dry-running or recovering a mini_racer_rs release; when editing release-please-config.json, .github/workflows/release.yml or release-please.yml, rakelib/package.rake, release/, or the publish, floor, size and lockfile scripts; when a release PR, a compile leg, an install test or the RubyGems.org publish fails; and before the first tag push (the RubyGems.org trusted publisher).
---

# Release Flow

A release is a merged release-please PR. Merging it tags `vX.Y.Z` and publishes a GitHub release, and that `release: published` event runs `.github/workflows/release.yml`: one native gem per platform, each carrying an extension for every Ruby in `tools/versions.toml` `[release] rubies`, plus the source gem; every gem installed into an empty `GEM_HOME` and smoke-tested; `SHA256SUMS` and build provenance; then, once the `rubygems` environment's required reviewer approves, a trusted-publishing push to RubyGems.org and an upload to the GitHub release.

## Quick reference

| Task | How |
|---|---|
| Dry run of the whole pipeline | `gh workflow run Release --ref main`, then `gh run watch <id>`; gems land on the run as `release-<version>` |
| Test a pipeline change | its pull request runs `.github/workflows/release.yml` as a dry run when it touches that workflow, the setup action, the gemspec, the loader, packaging, `release/`, the scripts the release runs or `tools/versions.toml` |
| Gem sizes | the Checksums job's summary, or `scripts/check-gem-sizes.sh <dir>` |
| Hold releases | any `RELEASE_HOLD` other than `false`, unset included: release-please keeps its PR current but never merges it |
| Release | `gh variable set RELEASE_HOLD --body false`, then `gh workflow run "Release Please"`; approve the `rubygems` deployment when the publish job waits |
| Publish a tag by hand | `gh workflow run Release --ref vX.Y.Z -f publish=true` |
| Native gem locally | `bundle exec rake compile`, `bundle exec rake package:stage PLATFORM=<platform>` per Ruby, then `bundle exec rake package:native PLATFORM=<platform>` |
| Source gem locally | `SOURCE_DATE_EPOCH=$(git log -1 --format=%ct) bundle exec rake package:source` |
| What a publish would push | `scripts/publish-gems.sh --dry-run <dir>` |
| Lockfiles after a version bump | `scripts/sync-release-locks.sh` |
| Smoke-test an installed gem | `EXPECTED_PLATFORM=<platform or ruby> ruby release/smoke.rb`, from outside the checkout |

## Before the first release

1. The release-kun App secrets `RELEASE_KUN_APP_ID` and `RELEASE_KUN_APP_PRIVATE_KEY`, as Actions and Dependabot secrets. Without them release-please opens its PR with `GITHUB_TOKEN`: CI never runs on it, its lockfiles stay unsynced, and the tag it makes starts no workflow, so publishing needs "Publish a tag by hand".
2. A dry run on `main`, then a committed release/size-budget.toml with `max_bytes` set to the largest gem times 1.15, at most 75,000,000.
3. Within 12 hours before the first tag: a pending trusted publisher on RubyGems.org for `mini_racer_rs`, repository andymai/mini_racer_rs, workflow file release.yml, environment `rubygems`. Pending publishers expire after 12 hours.
4. Only then set `RELEASE_HOLD` to `false`.

## When something goes wrong

| Symptom | Meaning | Next step |
|---|---|---|
| Verify: "version.rb is at X but the tag is Y" | The tag does not match the version file | Never retag by hand; let release-please make the tag |
| check-versions: "locks mini_racer_rs at ..." | A release PR whose lockfiles were not synced | `scripts/sync-release-locks.sh` on the PR branch, commit, push |
| `VIOLATION [glibc]` | The extension needs a newer glibc than `[release] glibc` | Find the symbol in the message; the Linux legs must build on `ubuntu-22.04` |
| `VIOLATION [macos]` | The bundle's minimum macOS is above `[release] macos` | `MACOSX_DEPLOYMENT_TARGET` comes from that floor; a dependency built for a newer macOS raises it |
| One install leg fails | That platform and Ruby cannot load its gem | Reproduce with "Native gem locally" and `release/smoke.rb` |
| A size violation | A gem is over the budget in release/size-budget.toml | Find what grew; raising the budget, or passing 75 MB, is the owner's decision |
| Publish failed part-way | Some gems are on RubyGems.org | Rerun the publish job: `scripts/publish-gems.sh` skips gems already there |
| A broken gem was published | RubyGems.org never takes a version twice | `gem yank mini_racer_rs -v X.Y.Z --platform <platform>`, then a fix release |

Details: [reference.md](reference.md).
