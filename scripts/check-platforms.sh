#!/usr/bin/env bash
# The supported targets are listed in several files; they must name the same ones.
set -euo pipefail
cd "$(dirname "$0")/.."

targets="aarch64-apple-darwin aarch64-unknown-linux-gnu aarch64-unknown-linux-musl x86_64-apple-darwin x86_64-unknown-linux-gnu x86_64-unknown-linux-musl"
gem_platforms="aarch64-linux-gnu arm64-darwin x86_64-darwin x86_64-linux-gnu"
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
release_include=$(sed -nE 's/^ *- platform: ([a-z0-9_-]+)$/\1/p' .github/workflows/release.yml | xargs)
release_lists=$(sed -nE 's/^ *platform: \[(.+)\]$/\1/p' .github/workflows/release.yml | tr -d ',')
package=$(sed -nE 's/^ *PLATFORMS = %w\[(.+)\]\.freeze$/\1/p' rakelib/package.rake)
publish=$(sed -nE 's/^platforms="(.+)"$/\1/p' scripts/publish-gems.sh)
lock=$(platforms_of Gemfile.lock)
transitive=$(platforms_of test/consumers/transitive/Gemfile.lock)

same "$archives" "$targets" || mismatch "v8-archives.sha256 archives: $archives"
same "$bindings" "$targets" || mismatch "v8-archives.sha256 bindings: $bindings"
same "$extconf" "$targets" || mismatch "extconf.rb targets: $extconf"
same "$deny" "$targets" || mismatch "deny.toml graph targets: $deny"
same "$about" "$targets" || mismatch "about.toml targets: $about"
same "$release_include" "$gem_platforms" || mismatch "release.yml matrix includes: $release_include"
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
