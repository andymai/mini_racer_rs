#!/bin/sh
# musl builds and tests inside a ruby:<version>-alpine container, started as
# root with the checkout as its working directory, so CI, the release and a
# local podman run do the same thing. The setup action's Ruby, cache and tools
# are glibc builds, which is why these run in a container.
#   scripts/alpine.sh test                 compile, then the Ruby tests, both strict upstream suites and the madvise fixture
#   scripts/alpine.sh compile <platform>   compile with the caller's release environment, smoke-test, stage for package:native
#   scripts/alpine.sh install <platform> [--smoke-only]
#                                          install pkg/*.gem with no compiler and smoke-test it, then run the suites on it
# SEED, when set, seeds the upstream suites.
set -eu
cd "$(dirname "$0")/.."

export CARGO_HOME="${CARGO_HOME:-/usr/local/cargo}" RUSTUP_HOME="${RUSTUP_HOME:-/usr/local/rustup}"
export PATH="$CARGO_HOME/bin:$PATH"
owner=$(stat -c %u .)

step() {
  echo "==> $*"
  "$@"
}

# What writes to the checkout runs as its owner, so a runner's or a podman
# user's files stay theirs, and so the tests run unprivileged: root ignores
# the RLIMIT_NPROC a watchdog test sets.
as_owner() {
  echo "==> $*"
  if [ -z "$user" ]; then
    "$@"
  else
    HOME=/home/owner su -m -s /bin/sh -c '"$@"' -- "$user" sh "$@"
  fi
}

user=
if [ "$owner" -ne 0 ]; then
  user=$(awk -F: -v uid="$owner" '$3 == uid { print $1; exit }' /etc/passwd)
  if [ -z "$user" ]; then
    adduser -D -u "$owner" -h /home/owner owner
    user=owner
  fi
  mkdir -p /home/owner
  chown "$owner" /home/owner
fi

# What the suites need: a compiler for the gems' own extensions, bash for the
# repository's scripts, and libffi for fiddle on Ruby 4.0.
packages() {
  step apk add --no-cache bash build-base diffutils libffi-dev linux-headers "$@"
}

gems() {
  step env BUNDLE_FROZEN=true bundle install --jobs 4
}

toolchain() {
  packages clang-dev curl rustup
  step rustup-init -y --profile minimal --default-toolchain none --no-modify-path
  step rustup toolchain install
  # cargo, run as the owner, keeps its registry and locks here.
  mkdir -p "$CARGO_HOME/registry"
  chown "$owner" "$CARGO_HOME" "$CARGO_HOME/registry"
  # Fetched as root, read by cargo as the owner, who cannot read /root.
  export MINI_RACER_RS_V8_CACHE="${MINI_RACER_RS_V8_CACHE:-/var/cache/mini_racer_rs/v8}"
  # shellcheck source=scripts/fetch-v8-archive.sh
  . scripts/fetch-v8-archive.sh
  gems
}

ruby_tests() {
  as_owner bundle exec ruby -w -Ilib -Itest/ruby -e 'Dir["test/ruby/**/*_test.rb"].sort.each { require File.expand_path(_1) }'
}

upstream() {
  as_owner bundle exec ruby test/upstream/run.rb --strict ${SEED:+--seed "$SEED"}
  as_owner bundle exec ruby test/upstream/run.rb --suite execjs --strict ${SEED:+--seed "$SEED"}
}

test_build() {
  export CI=true
  toolchain
  as_owner bundle exec rake compile
  as_owner scripts/check-exports.sh
  ruby_tests
  upstream
  # Plain cargo has none of extconf's flags, and cargo refuses a musl cdylib
  # without this one.
  as_owner env RUSTFLAGS="${RUSTFLAGS:-} -C target-feature=-crt-static" scripts/madvise-fixture.sh
}

# Build paths stay out of the extension's panic messages.
compile() {
  toolchain
  as_owner env RUSTFLAGS="--remap-path-prefix=$PWD=/mini_racer_rs --remap-path-prefix=$CARGO_HOME=/cargo" bundle exec rake compile
  as_owner ruby -Ilib -e 'require "mini_racer"; exit(MiniRacer::Context.new.eval("1+1") == 2)'
  as_owner bundle exec rake package:stage PLATFORM="$1"
}

# The gem goes into an empty GEM_HOME before anything that compiles is
# installed, so it loads as a stock image's Ruby finds it. The suites then run
# on that installed extension.
install() {
  for tool in cc gcc clang cargo rustc; do
    if command -v "$tool" >/dev/null; then
      echo "alpine: $tool is installed; the gem must install without a compiler" >&2
      exit 1
    fi
  done
  root=$PWD
  (
    export GEM_HOME=/tmp/gems GEM_PATH=/tmp/gems
    step gem install --local --no-document pkg/*.gem
    cd /tmp
    step env EXPECTED_PLATFORM="$1" ruby "$root/release/smoke.rb"
  )
  if [ "${2:-}" = --smoke-only ]; then
    return
  fi
  as_owner env GEM_HOME=/tmp/gems GEM_PATH=/tmp/gems ruby -rfileutils -e '
    minor = RUBY_VERSION[/\A\d+\.\d+/]
    gem_dir = Gem::Specification.find_by_name("mini_racer_rs").full_gem_path
    from = File.join(gem_dir, "lib", "mini_racer_rs", minor, "mini_racer_rs.so")
    to = File.join("lib", "mini_racer_rs", minor)
    FileUtils.mkdir_p(to)
    FileUtils.cp(from, to)
    puts "#{from} -> #{to}"
  '
  export CI=true
  packages
  gems
  ruby_tests
  upstream
}

mode=${1:-}
[ $# -eq 0 ] || shift
case "$mode" in
  test) test_build ;;
  compile) compile "${1:?usage: alpine.sh compile <platform>}" ;;
  install) install "${1:?usage: alpine.sh install <platform> [--smoke-only]}" "${2:-}" ;;
  *)
    echo "usage: $0 test | compile <platform> | install <platform> [--smoke-only]" >&2
    exit 2
    ;;
esac
