#!/usr/bin/env bash
# The supported targets are listed in several files; they must name the same four.
set -euo pipefail
cd "$(dirname "$0")/.."

targets="aarch64-apple-darwin aarch64-unknown-linux-gnu x86_64-apple-darwin x86_64-unknown-linux-gnu"
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
extconf=$(sed -nE 's/^ *%w\[[a-z0-9_]+ [a-z]+\] => "([^"]+)",$/\1/p' ext/mini_racer_rs/extconf.rb | xargs)
deny=$(taplo get -f deny.toml -o json | jq -r '.graph.targets[]' | xargs)
lock=$(sed -n '/^PLATFORMS$/,/^$/p' Gemfile.lock | sed -nE 's/^  (.+)$/\1/p' | xargs)

same "$archives" "$targets" || mismatch "v8-archives.sha256 archives: $archives"
same "$bindings" "$targets" || mismatch "v8-archives.sha256 bindings: $bindings"
same "$extconf" "$targets" || mismatch "extconf.rb targets: $extconf"
same "$deny" "$targets" || mismatch "deny.toml graph targets: $deny"
for platform in $gem_platforms; do
  grep -qw -- "$platform" <<<"$lock" || mismatch "Gemfile.lock PLATFORMS lacks $platform"
done

if [ "$fail" -ne 0 ]; then
  exit 1
fi
echo "Platform lists agree."
