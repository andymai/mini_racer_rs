#!/usr/bin/env bash
# A macOS extension must load on the oldest macOS the native gems support: its
# minimum OS version must not be newer than the floor in tools/versions.toml.
# Usage: check-macos-minos.sh <path/to/mini_racer_rs.bundle>
set -euo pipefail
cd "$(dirname "$0")/.."

lib=${1:?usage: check-macos-minos.sh <path/to/mini_racer_rs.bundle>}
floor=$(taplo get -f tools/versions.toml release.macos)
minos=$(otool -l "$lib" | awk '$1 == "cmd" && $2 == "LC_BUILD_VERSION" { found = 1 } found && $1 == "minos" { print $2; exit }')
[ -n "$minos" ] || { echo "$lib has no LC_BUILD_VERSION minos" >&2; exit 1; }
newest=$(printf '%s\n%s\n' "$minos" "$floor" | sort -V | tail -n 1)
if [ "$newest" != "$floor" ]; then
  echo "VIOLATION [macos]: $lib needs macOS $minos, newer than the floor $floor"
  exit 1
fi
echo "$lib needs macOS $minos, within the floor $floor."
