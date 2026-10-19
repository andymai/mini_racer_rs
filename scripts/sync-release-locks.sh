#!/usr/bin/env bash
# Brings the lockfiles to MiniRacerRs::VERSION once release-please has bumped
# it: the gem's own entries in both Gemfile.locks, the workspace crates in
# Cargo.lock, and mini_racer_wire and mini_racer_sourcemap in the fuzz crate's
# own Cargo.lock.
# release-please edits version files but never runs a resolver.
set -euo pipefail
cd "$(dirname "$0")/.."

version=$(sed -nE 's/^ *VERSION = "([^"]+)"$/\1/p' lib/mini_racer_rs/version.rb)
[ -n "$version" ] || { echo "no VERSION in lib/mini_racer_rs/version.rb" >&2; exit 1; }
for lock in Gemfile.lock test/consumers/transitive/Gemfile.lock; do
  VERSION="$version" ruby -i -pe \
    'sub(/\A(\s+mini_racer_rs \((?:= )?)[^)]+\)$/) { "#{$1}#{ENV.fetch("VERSION")})" }' "$lock"
done
cargo update --workspace --quiet
cargo update -p mini_racer_wire -p mini_racer_sourcemap --manifest-path crates/mini_racer_wire/fuzz/Cargo.toml --quiet
