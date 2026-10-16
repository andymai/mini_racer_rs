#!/usr/bin/env bash
# A Linux extension must load on the oldest glibc the native gems support: no
# symbol may need a newer GLIBC_ version than the floor in tools/versions.toml.
# Usage: check-glibc-floor.sh <path/to/mini_racer_rs.so>
set -euo pipefail
cd "$(dirname "$0")/.."

lib=${1:?usage: check-glibc-floor.sh <path/to/mini_racer_rs.so>}
floor=$(taplo get -f tools/versions.toml release.glibc)
needed=$(objdump -T "$lib" | grep -oE 'GLIBC_[0-9]+(\.[0-9]+)+' | sed 's/^GLIBC_//' | sort -uV | tail -n 1)
[ -n "$needed" ] || { echo "$lib names no GLIBC_ symbol versions" >&2; exit 1; }
newest=$(printf '%s\n%s\n' "$needed" "$floor" | sort -V | tail -n 1)
if [ "$newest" != "$floor" ]; then
  echo "VIOLATION [glibc]: $lib needs glibc $needed, newer than the floor $floor:"
  objdump -T "$lib" | grep -E "GLIBC_$needed\b" | awk '{ print "  " $NF }'
  exit 1
fi
echo "$lib needs glibc $needed, within the floor $floor."
