#!/usr/bin/env bash
# Runs every fuzz target for N seconds (default 60) on the nightly pinned
# as toolchains.fuzz in tools/versions.toml, installing it when missing.
# Each target reads and grows <corpus dir>/<target> when a corpus directory is
# given, else cargo fuzz's own fuzz/corpus/<target>. decode_arbitrary also
# starts from the frames V8 wrote in crates/mini_racer_wire/tests/golden.
# Usage: fuzz-smoke.sh [seconds] [corpus dir]
set -euo pipefail
cd "$(dirname "$0")/.."

seconds="${1:-60}"
corpus="${2:-}"
toolchain=$(sed -n '/^\[toolchains\]$/,/^\[/{s/^fuzz = "\([^"]*\)"$/\1/p}' tools/versions.toml)
[ -n "$toolchain" ] || { echo "fuzz-smoke: tools/versions.toml pins no toolchains.fuzz" >&2; exit 1; }
rustup run "$toolchain" rustc --version >/dev/null 2>&1 || rustup toolchain install "$toolchain" --profile minimal
if [ -n "$corpus" ]; then
  mkdir -p "$corpus"
  corpus=$(cd "$corpus" && pwd)
fi

cd crates/mini_racer_wire
# cargo fuzz has no --locked, so a stale fuzz/Cargo.lock fails here instead.
cargo "+$toolchain" fetch --locked --manifest-path fuzz/Cargo.toml
targets=$(cargo "+$toolchain" fuzz list)
[ -n "$targets" ] || { echo "fuzz-smoke: cargo fuzz list found no targets" >&2; exit 1; }
# cargo fuzz defaults to the triple it was itself built for, which for a
# prebuilt binary can be musl.
host=$(rustc "+$toolchain" -vV | sed -n 's/^host: //p')
# A seed directory of its own: libFuzzer writes what it finds to the first
# corpus directory only, so the golden files stay out of the cached corpus.
seeds=$(mktemp -d)
trap 'rm -rf "$seeds"' EXIT
cp tests/golden/*.bin "$seeds/"
for target in $targets; do
  dirs=("${corpus:-fuzz/corpus}/$target")
  mkdir -p "${dirs[0]}"
  [ "$target" != decode_arbitrary ] || dirs+=("$seeds")
  echo "==> fuzz $target (${seconds}s, corpus ${dirs[0]})"
  cargo "+$toolchain" fuzz run --target "$host" "$target" "${dirs[@]}" -- -max_total_time="$seconds"
done
