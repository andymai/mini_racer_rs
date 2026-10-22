#!/usr/bin/env bash
# Pushes a release's gems to RubyGems.org, the platform gems first and the
# source gem last, so no install meets a version that has only the source
# gem. A gem RubyGems.org already has at this version and platform is
# skipped, so a rerun after a partial publish finishes it. Expects the
# credentials the trusted publishing step exports. With --sign, each gem is
# signed through Sigstore with the job's OIDC identity and pushed with that
# attestation, as rubygems/release-gem does; a gem that fails to sign, or
# whose signed push fails, is pushed without one so a release never waits
# on Sigstore.
# Usage: publish-gems.sh [--dry-run] [--sign] <dir with .gem files>
set -euo pipefail
cd "$(dirname "$0")/.."

dry_run=false
sign=false
while [ $# -gt 0 ]; do
  case "$1" in
    --dry-run) dry_run=true ;;
    --sign) sign=true ;;
    *) break ;;
  esac
  shift
done
dir=${1:?usage: publish-gems.sh [--dry-run] [--sign] <dir with .gem files>}
platforms="aarch64-linux-gnu arm64-darwin x86_64-darwin x86_64-linux-gnu"
version=$(sed -nE 's/^ *VERSION = "([^"]+)"$/\1/p' lib/mini_racer_rs/version.rb)

order=()
for platform in $platforms; do
  order+=("$platform $dir/mini_racer_rs-$version-$platform.gem")
done
order+=("ruby $dir/mini_racer_rs-$version.gem")
expected=$(for entry in "${order[@]}"; do echo "${entry#* }"; done | sort)
found=$(find "$dir" -maxdepth 1 -name '*.gem' | sort)
if [ "$found" != "$expected" ]; then
  echo "expected exactly these gems in $dir:" >&2
  echo "$expected" >&2
  echo "found:" >&2
  echo "$found" >&2
  exit 1
fi

versions=$(mktemp)
code=$(curl -sS -o "$versions" -w '%{http_code}' https://rubygems.org/api/v1/versions/mini_racer_rs.json)
case "$code" in
  200) ;;
  404) echo '[]' >"$versions" ;;
  *) echo "RubyGems.org answered $code for the version list" >&2; exit 1 ;;
esac

for entry in "${order[@]}"; do
  platform=${entry%% *}
  gem=${entry#* }
  if jq -e --arg version "$version" --arg platform "$platform" \
    'any(.[]; .number == $version and .platform == $platform)' "$versions" >/dev/null; then
    echo "$(basename "$gem") is already on RubyGems.org; skipped."
    continue
  fi
  if [ "$dry_run" = true ]; then
    echo "would push $(basename "$gem")"
    continue
  fi
  if [ "$sign" = true ]; then
    if gem exec sigstore-cli:0.2.3 sign "$gem" --bundle "$gem.sigstore.json"; then
      gem push --attestation "$gem.sigstore.json" "$gem" && continue
      echo "::warning::pushing $(basename "$gem") with its attestation failed; pushing it without one"
    else
      echo "::warning::signing $(basename "$gem") failed; pushing it without an attestation"
    fi
    rm -f "$gem.sigstore.json"
  fi
  gem push "$gem"
done
