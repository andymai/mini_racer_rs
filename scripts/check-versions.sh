#!/usr/bin/env bash
# Version literals that must agree across Cargo, the gem, the shim and tooling.
set -euo pipefail
cd "$(dirname "$0")/.."

fail=0
mismatch() {
  echo "VIOLATION [version]: $*"
  fail=1
}

workspace=$(taplo get -f Cargo.toml workspace.package.version)
gem=$(sed -nE 's/^ *VERSION = "([^"]+)"$/\1/p' lib/mini_racer_rs/version.rb)
[ "$workspace" = "$gem" ] || mismatch "Cargo workspace $workspace != MiniRacerRs::VERSION $gem"
if [ -f .release-please-manifest.json ]; then
  manifest=$(jq -r '."."' .release-please-manifest.json)
  [ "$manifest" = "$gem" ] || mismatch "release-please manifest $manifest != MiniRacerRs::VERSION $gem"
fi

explicit=$(grep -lE '^version = ' crates/*/Cargo.toml ext/*/Cargo.toml || true)
[ -z "$explicit" ] || mismatch "crates must use version.workspace = true: $explicit"

parity=$(sed -nE 's/^ *VERSION = "([^"]+)"$/\1/p' lib/mini_racer/version.rb)
shim=$(sed -nE 's/^ *spec\.version = "([^"]+)"$/\1/p' shim/mini_racer.gemspec)
[ "$parity" = "$shim" ] || mismatch "MiniRacer::VERSION $parity != shim version $shim"
if [ -d vendor/mini_racer/.git ] || [ -f vendor/mini_racer/.git ]; then
  tag=$(git -C vendor/mini_racer describe --tags --exact-match 2>/dev/null || echo none)
  [ "$tag" = "v$parity" ] || mismatch "vendor/mini_racer is at $tag, expected v$parity"
fi

msrv=$(taplo get -f Cargo.toml workspace.package.rust-version)
tool_msrv=$(taplo get -f tools/versions.toml toolchains.msrv)
extconf_msrv=$(sed -nE 's/^MINIMUM_RUST = Gem::Version.new\("([^"]+)"\)$/\1/p' ext/mini_racer_rs/extconf.rb)
[ "${tool_msrv%.*}" = "$msrv" ] || mismatch "tools/versions.toml msrv $tool_msrv != rust-version $msrv"
[ "${extconf_msrv%.*}" = "$msrv" ] || mismatch "extconf MINIMUM_RUST $extconf_msrv != rust-version $msrv"

rb_sys_crate=$(awk '/^name = "rb-sys"$/ { getline; gsub(/version = |"/, ""); print }' Cargo.lock)
rb_sys_gem=$(sed -nE 's/^ *spec\.add_dependency "rb_sys", "= ([^"]+)"$/\1/p' mini_racer_rs.gemspec)
[ "$rb_sys_crate" = "$rb_sys_gem" ] || mismatch "rb-sys crate $rb_sys_crate != rb_sys gem $rb_sys_gem"

v8_pin=$(taplo get -f Cargo.toml workspace.dependencies.v8)
v8_lock=$(awk '/^name = "v8"$/ { getline; gsub(/version = |"/, ""); print }' Cargo.lock)
[ "$v8_pin" = "=$v8_lock" ] || mismatch "workspace v8 pin $v8_pin != Cargo.lock $v8_lock"

if [ "$fail" -ne 0 ]; then
  exit 1
fi
echo "Versions agree."
