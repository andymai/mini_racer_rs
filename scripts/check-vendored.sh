#!/usr/bin/env bash
# The TruffleRuby backend is upstream's own code, vendored byte for byte.
set -euo pipefail
cd "$(dirname "$0")/.."

[ -f vendor/mini_racer/lib/mini_racer/shared.rb ] || { echo "vendor/mini_racer is empty; run git submodule update --init" >&2; exit 2; }
fail=0
for file in lib/mini_racer/truffleruby.rb lib/mini_racer/shared.rb; do
  if ! cmp -s "$file" "vendor/mini_racer/$file"; then
    echo "VIOLATION [vendored]: $file differs from vendor/mini_racer/$file"
    fail=1
  fi
done
if ! cmp -s licenses/mini_racer.txt vendor/mini_racer/LICENSE.txt; then
  echo "VIOLATION [vendored]: licenses/mini_racer.txt differs from vendor/mini_racer/LICENSE.txt"
  fail=1
fi
if [ "$fail" -ne 0 ]; then
  exit 1
fi
echo "Vendored upstream files match vendor/mini_racer."
