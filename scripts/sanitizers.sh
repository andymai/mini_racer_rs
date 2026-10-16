#!/usr/bin/env bash
# AddressSanitizer and ThreadSanitizer runs on the pinned nightly, Linux x86_64:
# engine teardown with leak detection in both V8 platform modes, every engine
# and sync test for memory errors, and the sync crate's tests for data races.
set -euo pipefail
cd "$(dirname "$0")/.."

toolchain=$(taplo get -f tools/versions.toml toolchains.fuzz)
target=x86_64-unknown-linux-gnu
rustup toolchain list | grep -q "^$toolchain" || rustup toolchain install "$toolchain" --profile minimal
rustup component add rust-src --toolchain "$toolchain"
# The leak suppression matches function names, so stacks must be symbolized.
symbolizer=$(command -v llvm-symbolizer || find /usr/bin -name 'llvm-symbolizer-*' | sort -V | tail -1)
[ -n "$symbolizer" ] || { echo "sanitizers: llvm-symbolizer not found" >&2; exit 1; }
export ASAN_SYMBOLIZER_PATH="$symbolizer"
# shellcheck source=scripts/fetch-v8-archive.sh
. scripts/fetch-v8-archive.sh

asan() {
  RUSTFLAGS="-Zsanitizer=address" cargo "+$toolchain" test --locked --target "$target" --target-dir target/asan "$@"
}
ASAN_OPTIONS=detect_leaks=1 LSAN_OPTIONS="suppressions=$PWD/.github/sanitizers/lsan.supp" \
  asan -p mini_racer_engine --test teardown --test teardown_single_threaded
# A dropped handle shuts its runner down asynchronously, so other test processes
# can exit with isolates still alive; only the teardown tests above check leaks.
ASAN_OPTIONS=detect_leaks=0 asan -p mini_racer_engine -p mini_racer_sync --lib --tests
RUSTFLAGS="-Zsanitizer=thread" TSAN_OPTIONS=halt_on_error=1 \
  cargo "+$toolchain" test -Zbuild-std --locked -p mini_racer_sync --lib --target "$target" --target-dir target/tsan
echo "Sanitizer runs are clean."
