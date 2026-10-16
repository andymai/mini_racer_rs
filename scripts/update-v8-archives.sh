#!/usr/bin/env bash
# Rewrites ext/mini_racer_rs/v8-archives.sha256 for the v8 version in Cargo.lock:
# downloads the prebuilt archive and source binding for every supported target
# into the cache scripts/fetch-v8-archive.sh reads, and records their digests.
# rusty_v8 publishes no checksums, so the digests pin what was downloaded here.
set -euo pipefail
cd "$(dirname "$0")/.."

version=$(awk '/^name = "v8"$/ { getline; gsub(/version = |"/, ""); print }' Cargo.lock)
base="https://github.com/denoland/rusty_v8/releases/download/v$version"
cache="${MINI_RACER_RS_V8_CACHE:-${XDG_CACHE_HOME:-$HOME/.cache}/mini_racer_rs/v8}/v$version"
targets=$(taplo get -f deny.toml -o json | jq -r '.graph.targets[]')
[ -n "$targets" ] || { echo "update-v8-archives: deny.toml lists no targets" >&2; exit 1; }
mkdir -p "$cache"

pins=$(mktemp)
trap 'rm -f "$pins"' EXIT
for target in $targets; do
  for file in "librusty_v8_release_$target.a.gz" "src_binding_release_$target.rs"; do
    curl -fsSL --retry 3 -o "$cache/$file.part" "$base/$file"
    mv "$cache/$file.part" "$cache/$file"
    if command -v sha256sum >/dev/null; then
      sha256sum "$cache/$file"
    else
      shasum -a 256 "$cache/$file"
    fi | awk -v f="$file" '{ print $1 "  " f }' >>"$pins"
  done
done
sort -k2 "$pins" >ext/mini_racer_rs/v8-archives.sha256
echo "Pinned rusty_v8 v$version for: $(tr '\n' ' ' <<<"$targets")"
