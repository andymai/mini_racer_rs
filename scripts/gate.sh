#!/usr/bin/env bash
# Local gate before a PR. --quick: static checks. Default: plus builds and tests.
# --full: plus the slower supply-chain, MSRV, packaging and planted-violation checks.
set -euo pipefail
cd "$(dirname "$0")/.."

mode="${1:-default}"
case "$mode" in --quick | default | --full) ;; *) echo "usage: $0 [--quick|--full]" >&2; exit 2 ;; esac

step() {
  echo "==> $*"
  "$@"
}

passed() {
  echo "Gate passed ($mode)."
  exit 0
}

for tool in cargo taplo jq bundle npx; do
  command -v "$tool" >/dev/null || { echo "gate: $tool is not installed" >&2; exit 1; }
done
# shellcheck source=scripts/fetch-v8-archive.sh
. scripts/fetch-v8-archive.sh

step cargo fmt --all -- --check
step cargo fmt --manifest-path crates/mini_racer_wire/fuzz/Cargo.toml -- --check
step taplo fmt --check
step bundle exec rubocop
step scripts/check-boundaries.sh
step scripts/check-unsafe-fences.sh
step scripts/check-ffi-guards.sh
step scripts/check-gc-roots.sh
step scripts/check-statics.sh
step scripts/check-prose.sh
step scripts/check-action-pins.sh
step scripts/check-versions.sh
step scripts/check-platforms.sh
step scripts/check-vendored.sh
step scripts/check-doc-paths.sh
step scripts/test-claude-hooks.sh
step cargo clippy --workspace --all-targets --locked -- -D warnings
[ "$mode" = --quick ] && passed

step cargo nextest run --workspace --exclude mini_racer_rs --locked
step cargo test --doc --workspace --exclude mini_racer_rs --locked
RUSTFLAGS="${RUSTFLAGS:-} --cfg loom -Dwarnings" step cargo test -p mini_racer_sync --test loom --release --locked --target-dir target/loom
step bundle exec rake compile
step scripts/check-exports.sh
step bundle exec rake test
step bundle exec rake compat:check
CHECK=1 step bundle exec rake compat:render
step bundle exec ruby test/upstream/run.rb --strict
[ "$mode" = default ] && passed

msrv=$(taplo get -f tools/versions.toml toolchains.msrv)
step scripts/test-boundaries.sh
step bundle exec ruby test/upstream/run.rb --scripts
step bundle exec ruby test/diff/run.rb --twin --strict
step cargo "+$msrv" check --workspace --all-targets --locked
step scripts/check-deny.sh
step cargo audit
step cargo audit -f crates/mini_racer_wire/fuzz/Cargo.lock
step cargo machete
step scripts/fuzz-smoke.sh 20
RUSTDOCFLAGS="-D warnings" step cargo doc --workspace --no-deps --locked
step gem build mini_racer_rs.gemspec --output tmp/mini_racer_rs.gem
step gem build shim/mini_racer.gemspec --output tmp/mini_racer-shim.gem
passed
