#!/usr/bin/env bash
# Pushes a release's gems to RubyGems.org, the platform gems first and the
# source gem last, so no install meets a version that has only the source
# gem. A gem RubyGems.org already has at this version and platform is
# skipped, so a rerun after a partial publish finishes it. Expects the
# credentials the trusted publishing step exports. A gem with the bundle
# scripts/sign-gems.sh writes next to it is pushed with that attestation;
# if that push fails, it is pushed without one so a release never waits on
# Sigstore.
# Usage: publish-gems.sh [--dry-run] <dir with .gem files>
set -euo pipefail
cd "$(dirname "$0")/.."

dry_run=false
if [ "${1:-}" = --dry-run ]; then
  dry_run=true
  shift
fi
dir=${1:?usage: publish-gems.sh [--dry-run] <dir with .gem files>}
platforms="aarch64-linux-gnu aarch64-linux-musl arm64-darwin x86_64-darwin x86_64-linux-gnu x86_64-linux-musl"
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
  if [ -f "$gem.sigstore.json" ]; then
    gem push --attestation "$gem.sigstore.json" "$gem" && continue
    echo "::warning::pushing $(basename "$gem") with its attestation failed; pushing it without one"
    rm -f "$gem.sigstore.json"
  fi
  gem push "$gem"
done
