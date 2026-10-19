#!/usr/bin/env bash
# Process-global state lives in engine::process so fork reconciliation can find
# all of it. The L0 crates hold no statics.
set -euo pipefail
cd "$(dirname "$0")/.."

pattern='^\s*(pub(\([a-z]+\))? )?static |thread_local!'
hits=$(
  {
    grep -rnE "$pattern" crates/mini_racer_engine/src |
      grep -vE '^crates/mini_racer_engine/src/process(\.rs|/)' || true
    grep -rnE "$pattern" crates/mini_racer_wire/src crates/mini_racer_sync/src crates/mini_racer_sourcemap/src || true
  }
)
if [ -n "$hits" ]; then
  sed 's/^/VIOLATION [static]: /' <<<"$hits"
  exit 1
fi
echo "Statics are confined to engine::process."
