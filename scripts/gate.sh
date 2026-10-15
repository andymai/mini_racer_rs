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

for tool in cargo taplo jq bundle npx; do
  command -v "$tool" >/dev/null || { echo "gate: $tool is not installed" >&2; exit 1; }
done
# shellcheck source=scripts/fetch-v8-archive.sh
. scripts/fetch-v8-archive.sh

step cargo fmt --all -- --check
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
step scripts/check-doc-paths.sh
step scripts/test-claude-hooks.sh
step cargo clippy --workspace --all-targets --locked -- -D warnings
[ "$mode" = --quick ] && exit 0

step cargo nextest run --workspace --exclude mini_racer_rs --locked
step cargo test --doc --workspace --exclude mini_racer_rs --locked
step bundle exec rake compile
step scripts/check-exports.sh
step bundle exec rake test
[ "$mode" = default ] && exit 0

msrv=$(taplo get -f tools/versions.toml toolchains.msrv)
step scripts/test-boundaries.sh
step cargo "+$msrv" check --workspace --all-targets --locked
step cargo deny check
step cargo audit
step cargo machete
RUSTDOCFLAGS="-D warnings" step cargo doc --workspace --no-deps --locked
step gem build mini_racer_rs.gemspec --output tmp/mini_racer_rs.gem
step gem build shim/mini_racer.gemspec --output tmp/mini_racer-shim.gem
echo "Gate passed ($mode)."
