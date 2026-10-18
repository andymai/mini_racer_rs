# Stability

mini_racer_rs is a pre-release Rust implementation of the MiniRacer Ruby API and a drop-in replacement for mini_racer 0.22.1. Nothing is published to RubyGems.org yet.

## Versions

`MiniRacerRs::VERSION` is the gem's version. It follows semantic versioning and starts below 1.0. Before 1.0, a breaking change increments the minor version. Releases are cut by release-please.

`MiniRacer::VERSION` remains `"0.22.1"`, and `MiniRacer::LIBV8_NODE_VERSION` retains mini_racer's value. These are parity constants, not the mini_racer_rs release version.

`MiniRacerRs::NATIVE_VERSION` reports the compiled extension's version. `MiniRacerRs::V8_VERSION` reports the compiled-in V8 version string.

The V8 version changes only in a minor release, never a patch, and the release notes call out the change and any message wording it changes. Security fixes land in the latest release only.

## Covered

The public API covers the `MiniRacer` namespace defined by mini_racer 0.22.1, plus the `MiniRacerRs` constants described above.

`COMPATIBILITY.md` lists the mini_racer 0.22.1 public surface and its status. Deliberate differences include a reason and a test. Compatibility is also checked against mini_racer's unmodified test suite.

The gem defines `MiniRacer` and provides `lib/mini_racer.rb`. Under Bundler, `require "mini_racer"` and ExecJS work unchanged. Loading mini_racer and mini_racer_rs together raises a `LoadError` naming the conflict.

## Not covered

`MINI_RACER_RS_SPIN_US` is an unstable tuning interface. Its format is `<microseconds with the GVL>,<microseconds without>`. `MINI_RACER_RS_CODE_CACHE` is an unstable tuning interface. Set it to `off` before the first context is created to disable caching; forked children retain the parent's setting. `MINI_RACER_RS_FAST_CALLS` is an unstable tuning interface. Set it to `off` before the first context is created to send every eval down the normal path; the setting is read once per process, and forked children retain it.

The internal Rust crates are unstable. Files under `lib/mini_racer_rs/` are unstable except for what `require "mini_racer"` and `require "mini_racer_rs"` load.

## Deprecation

Anything under `MiniRacerRs` and the `MINI_RACER_RS_SPIN_US` environment variable receive one minor release of notice before removal. The deprecating release keeps the affected interface working and warns when it is used. Removal occurs no earlier than the next minor release.

This guarantee covers removal only. `MINI_RACER_RS_SPIN_US` remains an unstable tuning interface, and its effect on timing can change in any release. The `MiniRacer` namespace continues to follow mini_racer 0.22.1.

## Platforms, Rubies and V8

Supported Rubies are CRuby 3.3, 3.4 and 4.0, plus TruffleRuby using mini_racer's vendored TruffleRuby backend. Supported Ruby versions follow CRuby's maintained releases.

Supported platforms are glibc Linux on x86_64 and aarch64, and macOS on arm64 and x86_64. musl-based Linux, including Alpine, and Windows are out of scope.

The extension uses V8 15.2 through the `v8` crate, pinned exactly at 152.2.0. Native gems contain one extension per supported Ruby. Source builds require Rust 1.91 or newer and libclang.
