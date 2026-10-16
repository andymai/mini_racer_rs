#!/usr/bin/env bash
# Builds the extension without the madvise wrapper and shows why the wrapper
# exists: a forked child that evaluates an inherited single-threaded context
# dies by a signal. Linux only, where V8 maps its memory MADV_DONTFORK.
set -euo pipefail
cd "$(dirname "$0")/.."

[ "$(uname -s)" = Linux ] || { echo "madvise-fixture: Linux only" >&2; exit 2; }
target="${CARGO_TARGET_DIR:-target}/no-madvise-wrap"
cargo build -p mini_racer_rs --features no-madvise-wrap --target-dir "$target" --locked
lib=$(mktemp -d)
trap 'rm -rf "$lib"' EXIT
cp -r lib/. "$lib"
# A native gem's per-Ruby extensions would win over the one this replaces.
find "$lib/mini_racer_rs" -mindepth 1 -maxdepth 1 -type d -exec rm -rf {} +
cp "$target/debug/libmini_racer_rs.so" "$lib/mini_racer_rs/mini_racer_rs.so"
ruby -I"$lib" test/stress/madvise_fixture.rb crash
