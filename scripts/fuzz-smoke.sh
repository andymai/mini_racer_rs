#!/usr/bin/env bash
# Runs every wire fuzz target for N seconds (default 60) on the pinned nightly.
set -euo pipefail
cd "$(dirname "$0")/.."

seconds="${1:-60}"
toolchain=$(sed -nE 's/^fuzz = "([^"]+)"$/\1/p' tools/versions.toml)
[ -n "$toolchain" ] || { echo "fuzz-smoke: tools/versions.toml pins no fuzz toolchain" >&2; exit 1; }
rustup run "$toolchain" rustc --version >/dev/null 2>&1 || {
  echo "fuzz-smoke: run bin/setup or: rustup toolchain install $toolchain --profile minimal" >&2
  exit 1
}

cd crates/mini_racer_wire
for target in $(cargo "+$toolchain" fuzz list); do
  echo "==> fuzz $target (${seconds}s)"
  cargo "+$toolchain" fuzz run "$target" -- -max_total_time="$seconds"
done
