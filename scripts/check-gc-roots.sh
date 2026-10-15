#!/usr/bin/env bash
# Invariant I3: Ruby values held in Rust storage go through ext::gc_root, which
# registers them with the GC. Other ways to keep a VALUE alive are confined there.
set -euo pipefail
cd "$(dirname "$0")/.."

hits=$(grep -rnE '\b(Lazy|Opaque|BoxValue)<|register_mark_object|rb_gc_register|gc::register' ext/mini_racer_rs/src |
  grep -v '^ext/mini_racer_rs/src/gc_root\.rs:' || true)
if [ -n "$hits" ]; then
  sed 's/^/VIOLATION [gc-root]: /' <<<"$hits"
  exit 1
fi
echo "Ruby values in Rust storage go through gc_root."
