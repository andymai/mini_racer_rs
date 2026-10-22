#!/usr/bin/env bash
# Signs each gem in a directory through Sigstore with the job's OIDC identity,
# writing <gem>.sigstore.json next to it, as rubygems/release-gem does, from
# the bundle locked with checksums in release/sign. The release workflow runs
# it in a job without the rubygems environment, so its OIDC token cannot be
# exchanged for a RubyGems.org key. A gem it cannot sign within 2 minutes is
# left without a bundle and pushed without an attestation, so a release
# never waits on Sigstore.
# Usage: sign-gems.sh <dir with .gem files>
set -euo pipefail
cd "$(dirname "$0")/.."

dir=${1:?usage: sign-gems.sh <dir with .gem files>}
# BUNDLE_VERSION=system: never download another Bundler, unchecked, to match
# the lockfile.
export BUNDLE_GEMFILE="$PWD/release/sign/Gemfile" BUNDLE_FROZEN=true BUNDLE_VERSION=system

if ! bundle install --quiet; then
  echo "::warning::installing release/sign failed; every gem is pushed without an attestation"
  exit 0
fi

for gem in "$dir"/*.gem; do
  if ! timeout 120 bundle exec sigstore-cli sign "$gem" --bundle "$gem.sigstore.json"; then
    echo "::warning::signing $(basename "$gem") failed; it is pushed without an attestation"
    rm -f "$gem.sigstore.json"
  fi
done
