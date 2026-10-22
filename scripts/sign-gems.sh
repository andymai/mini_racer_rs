#!/usr/bin/env bash
# Signs each gem in a directory through Sigstore with the job's OIDC identity,
# writing <gem>.sigstore.json next to it, as rubygems/release-gem does. The
# publish job runs it before RubyGems.org credentials exist in the job, from
# the bundle locked with checksums in release/sign. A gem it cannot sign is
# left without a bundle and pushed without an attestation, so a release
# never waits on Sigstore.
# Usage: sign-gems.sh <dir with .gem files>
set -euo pipefail
cd "$(dirname "$0")/.."

dir=${1:?usage: sign-gems.sh <dir with .gem files>}
export BUNDLE_GEMFILE="$PWD/release/sign/Gemfile" BUNDLE_FROZEN=true

if ! bundle install --quiet; then
  echo "::warning::installing release/sign failed; every gem is pushed without an attestation"
  exit 0
fi

for gem in "$dir"/*.gem; do
  if ! bundle exec sigstore-cli sign "$gem" --bundle "$gem.sigstore.json"; then
    echo "::warning::signing $(basename "$gem") failed; it is pushed without an attestation"
    rm -f "$gem.sigstore.json"
  fi
done
