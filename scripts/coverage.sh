#!/usr/bin/env bash
# Line and branch coverage on the nightly pinned as toolchains.fuzz, since
# branch coverage is unstable. Writes tmp/coverage/<suite>.lcov, llvm-cov's
# per-file report as <suite>.txt and a per-crate table as <suite>.md.
#   workspace  the Rust tests, as CI's Rust Tests job runs them
#   ruby       (Linux) the Ruby tests, the fork stress scripts and both upstream
#              suites on an instrumented extension, put in
#              lib/mini_racer_rs/<minor>/ for the run and removed after it
# Usage: coverage.sh workspace|ruby
set -euo pipefail
cd "$(dirname "$0")/.."

suite=${1:-}
case "$suite" in workspace | ruby) ;; *) echo "usage: $0 workspace|ruby" >&2; exit 2 ;; esac
toolchain=$(sed -n '/^\[toolchains\]$/,/^\[/{s/^fuzz = "\([^"]*\)"$/\1/p}' tools/versions.toml)
[ -n "$toolchain" ] || { echo "coverage: tools/versions.toml pins no toolchains.fuzz" >&2; exit 1; }
rustup toolchain list | grep -q "^$toolchain" || rustup toolchain install "$toolchain" --profile minimal
rustup component add llvm-tools --toolchain "$toolchain"
host=$(rustc "+$toolchain" -vV | sed -n 's/^host: //p')
llvm="$(rustc "+$toolchain" --print sysroot)/lib/rustlib/$host/bin"
out=tmp/coverage
mkdir -p "$out"
# shellcheck source=scripts/fetch-v8-archive.sh
. scripts/fetch-v8-archive.sh

# Sums lcov's line and branch counts over each crate's src/.
per_crate() {
  local rows
  rows=$(awk -v root="$PWD/" '
    /^SF:/ {
      crate = ""
      path = substr($0, 4)
      if (index(path, root) == 1 && split(substr(path, length(root) + 1), part, "/") > 3 &&
        (part[1] == "crates" || part[1] == "ext") && part[3] == "src") {
        crate = part[2]
        seen[crate] = 1
      }
    }
    crate != "" && /^LF:/ { lines[crate] += substr($0, 4) }
    crate != "" && /^LH:/ { hit[crate] += substr($0, 4) }
    crate != "" && /^BRF:/ { branches[crate] += substr($0, 5) }
    crate != "" && /^BRH:/ { taken[crate] += substr($0, 5) }
    END { for (c in seen) print c, hit[c] + 0, lines[c] + 0, taken[c] + 0, branches[c] + 0 }
  ' "$1" | sort)
  echo "| Crate | Lines | Line coverage | Branches | Branch coverage |"
  echo "|---|--:|--:|--:|--:|"
  awk '
    function pct(n, d) { return d ? sprintf("%.1f%%", 100 * n / d) : "-" }
    function row(name, h, l, t, b) { printf "| %s | %d/%d | %s | %d/%d | %s |\n", name, h, l, pct(h, l), t, b, pct(t, b) }
    NF == 5 { row($1, $2, $3, $4, $5); h += $2; l += $3; t += $4; b += $5 }
    END { row("all", h, l, t, b) }
  ' <<<"$rows"
}

status=0
if [ "$suite" = workspace ]; then
  cargo "+$toolchain" llvm-cov nextest --workspace --exclude mini_racer_rs --branch --locked --no-report
  cargo "+$toolchain" llvm-cov report --branch --lcov --output-path "$out/workspace.lcov"
  cargo "+$toolchain" llvm-cov report --branch >"$out/workspace.txt"
else
  [ "$(uname -s)" = Linux ] || { echo "coverage: the ruby suite runs on Linux only" >&2; exit 2; }
  ext="lib/mini_racer_rs/$(ruby -e 'print RUBY_VERSION[/\A\d+\.\d+/]')"
  # An extension there wins over every build, so it is this run's to remove.
  [ ! -e "$ext" ] || { echo "coverage: $ext exists; remove it first" >&2; exit 1; }
  eval "$(cargo "+$toolchain" llvm-cov show-env --sh --branch)"
  export RUSTUP_TOOLCHAIN="$toolchain"
  # Continuous mode maps the counters onto the profile file, so a forked child
  # that leaves through exit!, which skips the runtime's atexit writer, still
  # counts in its parent's file. On Linux it needs relocatable counters.
  export RUSTFLAGS="${RUSTFLAGS:+$RUSTFLAGS }-Cllvm-args=-runtime-counter-relocation"
  # Release, as CI builds it: under CI the stack cases expect release frames.
  cargo build -p mini_racer_rs --release --locked --target-dir target/coverage
  object="$out/mini_racer_rs.so"
  cp target/coverage/release/libmini_racer_rs.so "$object"
  trap 'rm -rf "$ext"' EXIT
  mkdir -p "$ext"
  cp "$object" "$ext/mini_racer_rs.so"
  rm -rf "$out/ruby"
  mkdir -p "$out/ruby"
  export LLVM_PROFILE_FILE="$PWD/$out/ruby/%p%c.profraw"

  # Every suite runs, so one failure still leaves a report of the others.
  run() {
    echo "==> $*"
    "$@" || status=1
  }
  # Without rake, whose test task compiles first.
  run bundle exec ruby -Ilib -Itest/ruby -e 'Dir["test/ruby/**/*_test.rb"].sort.each { require File.expand_path(_1) }'
  run bundle exec ruby -Ilib test/stress/fork_loop.rb
  run bundle exec ruby -Ilib test/stress/fork_busy.rb 300 4
  run bundle exec ruby test/upstream/run.rb --strict
  run bundle exec ruby test/upstream/run.rb --suite execjs --strict

  "$llvm/llvm-profdata" merge -sparse --failure-mode=all "$out"/ruby/*.profraw -o "$out/ruby.profdata"
  "$llvm/llvm-cov" export -format=lcov -instr-profile="$out/ruby.profdata" "$object" "$PWD/crates" "$PWD/ext" \
    >"$out/ruby.lcov"
  "$llvm/llvm-cov" report -show-branch-summary -instr-profile="$out/ruby.profdata" "$object" "$PWD/crates" "$PWD/ext" \
    >"$out/ruby.txt"
fi

per_crate "$out/$suite.lcov" >"$out/$suite.md"
cat "$out/$suite.md"
exit "$status"
