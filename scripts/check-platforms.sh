#!/usr/bin/env bash
# The supported targets are listed in several files; they must name the same ones.
set -euo pipefail
cd "$(dirname "$0")/.."

targets="aarch64-apple-darwin aarch64-unknown-linux-gnu aarch64-unknown-linux-musl x86_64-apple-darwin x86_64-unknown-linux-gnu x86_64-unknown-linux-musl"
gem_platforms="aarch64-linux-gnu aarch64-linux-musl arm64-darwin x86_64-darwin x86_64-linux-gnu x86_64-linux-musl"
fail=0
mismatch() {
  echo "VIOLATION [platform]: $*"
  fail=1
}
same() {
  [ "$(tr ' ' '\n' <<<"$1" | sort -u | xargs)" = "$(tr ' ' '\n' <<<"$2" | sort -u | xargs)" ]
}

archives=$(sed -nE 's/^[0-9a-f]{64}  librusty_v8_release_(.+)\.a\.gz$/\1/p' ext/mini_racer_rs/v8-archives.sha256 | xargs)
bindings=$(sed -nE 's/^[0-9a-f]{64}  src_binding_release_(.+)\.rs$/\1/p' ext/mini_racer_rs/v8-archives.sha256 | xargs)
extconf=$(sed -nE 's/^ *%w\[[a-z0-9_]+ [a-z-]+\] => "([^"]+)",$/\1/p' ext/mini_racer_rs/extconf.rb | xargs)
deny=$(taplo get -f deny.toml -o json | jq -r '.graph.targets[]' | xargs)
about=$(taplo get -f about.toml -o json | jq -r '.targets[]' | xargs)
platforms_of() {
  sed -n '/^PLATFORMS$/,/^$/p' "$1" | sed -nE 's/^  (.+)$/\1/p' | xargs
}
musl_platforms=$(tr ' ' '\n' <<<"$gem_platforms" | grep -- '-linux-musl$' | xargs)
release_includes=$(awk '/^  [a-z][a-z0-9-]*:$/ { job = $1 } /^ *- platform: [a-z0-9_-]+$/ { print job, $3 }' .github/workflows/release.yml)
release_lists=$(sed -nE 's/^ *platform: \[(.+)\]$/\1/p' .github/workflows/release.yml | tr -d ',')
package=$(sed -n '/^ *PLATFORMS = %w\[/,/\]\.freeze$/p' rakelib/package.rake | sed -E 's/^ *PLATFORMS = %w\[//; s/\]\.freeze$//' | xargs)
publish=$(sed -nE 's/^platforms="(.+)"$/\1/p' scripts/publish-gems.sh)
lock=$(platforms_of Gemfile.lock)
transitive=$(platforms_of test/consumers/transitive/Gemfile.lock)

same "$archives" "$targets" || mismatch "v8-archives.sha256 archives: $archives"
same "$bindings" "$targets" || mismatch "v8-archives.sha256 bindings: $bindings"
same "$extconf" "$targets" || mismatch "extconf.rb targets: $extconf"
same "$deny" "$targets" || mismatch "deny.toml graph targets: $deny"
same "$about" "$targets" || mismatch "about.toml targets: $about"
[ -n "$release_includes" ] || mismatch "release.yml has no matrix includes"
while read -r job; do
  included=$(awk -v job="$job" '$1 == job { print $2 }' <<<"$release_includes" | xargs)
  case "$job" in
    *musl*) expected=$musl_platforms ;;
    *) expected=$gem_platforms ;;
  esac
  same "$included" "$expected" || mismatch "release.yml ${job%:} matrix includes: $included"
done < <(cut -d' ' -f1 <<<"$release_includes" | sort -u)
while read -r list; do
  same "$(tr ' ' '\n' <<<"$list" | grep -vx ruby | xargs)" "$gem_platforms" || mismatch "release.yml matrix platforms: $list"
done <<<"$release_lists"
[ -n "$release_lists" ] || mismatch "release.yml has no platform matrix"
same "$package" "$gem_platforms" || mismatch "rakelib/package.rake PLATFORMS: $package"
same "$publish" "$gem_platforms" || mismatch "scripts/publish-gems.sh platforms: $publish"
for platform in $gem_platforms; do
  grep -qw -- "$platform" <<<"$lock" || mismatch "Gemfile.lock PLATFORMS lacks $platform"
  grep -qw -- "$platform" <<<"$transitive" || mismatch "test/consumers/transitive/Gemfile.lock PLATFORMS lacks $platform"
done

if [ "$fail" -ne 0 ]; then
  exit 1
fi
echo "Platform lists agree."
