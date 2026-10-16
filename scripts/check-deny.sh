#!/usr/bin/env bash
# cargo deny over the shipped graph (the supported targets in deny.toml), then
# over every target, the only way it sees cfg(loom) crates, then over the fuzz
# crate's own lockfile.
set -euo pipefail
cd "$(dirname "$0")/.."

cargo deny check
all_targets=$(mktemp)
trap 'rm -f "$all_targets"' EXIT
awk '/^\[/ { skip = ($0 == "[graph]") } !skip' deny.toml >"$all_targets"
cargo deny --config "$all_targets" check
cargo deny --manifest-path crates/mini_racer_wire/fuzz/Cargo.toml check
