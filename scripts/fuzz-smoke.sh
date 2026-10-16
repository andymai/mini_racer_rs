#!/usr/bin/env bash
# Runs every wire fuzz target for N seconds (default 60) on the nightly pinned
# as toolchains.fuzz in tools/versions.toml, installing it when missing.
set -euo pipefail
cd "$(dirname "$0")/.."

seconds="${1:-60}"
toolchain=$(sed -n '/^\[toolchains\]$/,/^\[/{s/^fuzz = "\([^"]*\)"$/\1/p}' tools/versions.toml)
[ -n "$toolchain" ] || { echo "fuzz-smoke: tools/versions.toml pins no toolchains.fuzz" >&2; exit 1; }
rustup run "$toolchain" rustc --version >/dev/null 2>&1 || rustup toolchain install "$toolchain" --profile minimal

cd crates/mini_racer_wire
# cargo fuzz has no --locked, so a stale fuzz/Cargo.lock fails here instead.
cargo "+$toolchain" fetch --locked --manifest-path fuzz/Cargo.toml
targets=$(cargo "+$toolchain" fuzz list)
[ -n "$targets" ] || { echo "fuzz-smoke: cargo fuzz list found no targets" >&2; exit 1; }
for target in $targets; do
  echo "==> fuzz $target (${seconds}s)"
  cargo "+$toolchain" fuzz run "$target" -- -max_total_time="$seconds"
done
