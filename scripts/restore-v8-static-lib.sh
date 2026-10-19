#!/usr/bin/env bash
# v8's build script unpacks RUSTY_V8_ARCHIVE into <target>/<profile>/gn_out/obj,
# beside the profile's deps rather than in its own build directory, and
# rust-cache does not keep that directory. A restored cache still holds the
# build script's recorded output, so a change below v8 that rebuilds the v8
# crate without rerunning its build script fails to link. This puts the library
# back wherever a recorded output points to one that is missing.
# Usage: restore-v8-static-lib.sh [checkout] (after sourcing fetch-v8-archive.sh)
set -euo pipefail

root=$(cd "${1:-$(dirname "$0")/..}" && pwd -P)
archive=${RUSTY_V8_ARCHIVE:?source scripts/fetch-v8-archive.sh first}

while IFS= read -r -d '' output; do
  dir=$(sed -nE 's/^cargo:rustc-link-search=(native=)?//p' "$output" | head -n 1)
  name=$(sed -nE 's/^cargo:rustc-link-lib=static=//p' "$output" | head -n 1)
  [ -n "$dir" ] && [ -n "$name" ] || continue
  lib="$dir/lib$name.a"
  [ -f "$lib" ] && continue
  case "$dir/" in
    "$root"/*) ;;
    *) echo "restore-v8-static-lib: $output links from $dir, outside $root; left alone" >&2; continue ;;
  esac
  mkdir -p "$dir"
  gzip -dc "$archive" >"$lib.part"
  mv "$lib.part" "$lib"
  echo "restored $lib"
done < <(find "$root" \( -path "$root/vendor" -o -path "$root/node_modules" -o -path "$root/.git" \) -prune -o -path '*/build/v8-*/output' -print0)
