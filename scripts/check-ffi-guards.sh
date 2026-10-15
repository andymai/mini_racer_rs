#!/usr/bin/env bash
# Invariant I4: every FFI entry point is panic-guarded. Ruby methods go through
# the export! macro, and every extern "C" fn body calls guard() or catch_unwind.
set -euo pipefail
cd "$(dirname "$0")/.."

fail=0

while IFS= read -r hit; do
  echo "VIOLATION [export]: $hit defines a Ruby method without export!"
  fail=1
done < <(grep -rnE '\b(magnus::)?(method|function)!\(' ext/mini_racer_rs/src |
  grep -vE '^ext/mini_racer_rs/src/lib\.rs:[0-9]+: +magnus::(method|function)!\(\$f, ' || true)

for file in $(grep -rlE 'extern "C" fn' crates/*/src ext/*/src 2>/dev/null || true); do
  awk -v file="$file" '
    /extern "C" fn/ { inside = 1; guarded = 0; start = FNR; name = $0 }
    inside && /(^|[^a-z_])(guard|catch_unwind)\(/ { guarded = 1 }
    inside && /^}/ {
      if (!guarded) { printf "VIOLATION [extern]: %s:%d extern \"C\" fn lacks a panic guard\n", file, start; bad = 1 }
      inside = 0
    }
    END { exit bad }
  ' "$file" || fail=1
done

if [ "$fail" -ne 0 ]; then
  exit 1
fi
echo "FFI entry points are panic-guarded."
