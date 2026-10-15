#!/usr/bin/env bash
# The extension's dynamic symbol table must equal the committed allowlist, carry
# no C++ or Rust mangled names, and (Linux) route V8's madvise to our wrapper.
# Usage: check-exports.sh [--write|--print] [path/to/mini_racer_rs.so|.bundle]
set -euo pipefail
cd "$(dirname "$0")/.."

mode=check
case "${1:-}" in
  --write | --print) mode="${1#--}"; shift ;;
esac

case "$(uname -s)" in
  Linux) os=linux dlext=so ;;
  Darwin) os=macos dlext=bundle ;;
  *) echo "unsupported OS $(uname -s)" >&2; exit 2 ;;
esac
lib="${1:-lib/mini_racer_rs/mini_racer_rs.$dlext}"
allow="ext/mini_racer_rs/exports.allow.$os"
[ -f "$lib" ] || { echo "$lib not found; run rake compile" >&2; exit 2; }

if [ "$os" = linux ]; then
  exports=$(nm -D --defined-only "$lib" | awk '{ print $NF }' | LC_ALL=C sort -u)
else
  exports=$(nm -gUj "$lib" | LC_ALL=C sort -u)
fi

if [ "$mode" = print ]; then
  printf '%s\n' "$exports"
  exit 0
fi
if [ "$mode" = write ]; then
  printf '%s\n' "$exports" >"$allow"
  echo "Wrote $(wc -l <"$allow" | tr -d ' ') symbols to $allow."
  exit 0
fi

fail=0
mangled=$(grep -E '^_{0,2}(ZN|R[A-Z0-9_])' <<<"$exports" || true)
if [ -n "$mangled" ]; then
  sed 's/^/VIOLATION [mangled]: /' <<<"$mangled"
  fail=1
fi
if ! diff <(printf '%s\n' "$exports") "$allow" >/dev/null; then
  echo "VIOLATION [allowlist]: exports differ from $allow (< built, > allowed):"
  diff <(printf '%s\n' "$exports") "$allow" | grep -E '^[<>]' | head -40
  fail=1
fi
if [ "$os" = linux ]; then
  if nm -D --undefined-only "$lib" | awk '{ print $NF }' | grep -qx madvise; then
    echo "VIOLATION [madvise]: $lib still references madvise; V8's calls must bind to __wrap_madvise"
    fail=1
  fi
  if ! grep -qx __wrap_madvise <<<"$exports"; then
    echo "VIOLATION [madvise]: $lib does not define __wrap_madvise"
    fail=1
  fi
fi

if [ "$fail" -ne 0 ]; then
  exit 1
fi
echo "Exports match $allow ($(wc -l <"$allow" | tr -d ' ') symbols)."
