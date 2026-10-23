# Stability

mini_racer_rs is published on RubyGems.org as a Rust implementation of the MiniRacer Ruby API and a drop-in replacement for mini_racer 0.22.1.

## Versions

`MiniRacerRs::VERSION` is the gem's version. It follows semantic versioning, so a breaking change increments the major version. Releases are cut by release-please.

`MiniRacer::VERSION` remains `"0.22.1"`, and `MiniRacer::LIBV8_NODE_VERSION` retains mini_racer's value. These are parity constants, not the mini_racer_rs release version.

On CRuby, `MiniRacerRs::NATIVE_VERSION` reports the compiled extension's version and `MiniRacerRs::V8_VERSION` reports the compiled-in V8 version string.

The V8 version changes only in a minor release, never a patch, and the release notes call out the change and any message wording it changes. Security fixes land in the latest release only.

## Covered

The public API covers the `MiniRacer` namespace defined by mini_racer 0.22.1, plus `MiniRacerRs.register_source_map` and the `MiniRacerRs` constants described above.

`COMPATIBILITY.md` lists the mini_racer 0.22.1 public surface and its status. Deliberate differences include a reason and a test. Compatibility is also checked against mini_racer's unmodified test suite.

The gem defines `MiniRacer` and provides `lib/mini_racer.rb`. Under Bundler, `require "mini_racer"` and ExecJS work unchanged. Loading mini_racer and mini_racer_rs together raises a `LoadError` naming the conflict.

## Not covered

`MINI_RACER_RS_SPIN_US` is an unstable tuning interface. Its format is `<microseconds with the GVL>,<microseconds without>`. `MINI_RACER_RS_CODE_CACHE` is an unstable tuning interface. Set it to `off` before the first context is created to disable the process-wide code cache and shared copies of large sources; forked children retain the parent's setting. `MINI_RACER_RS_FAST_CALLS` is an unstable tuning interface. Set it to `off` before the first context is created to send every eval down the normal path; the setting is read once per process, and forked children retain it.

The internal Rust crates are unstable. Files under `lib/mini_racer_rs/` are unstable except for what `require "mini_racer"` and `require "mini_racer_rs"` load.

## Deprecation

Covered interfaces are removed or changed incompatibly only in a major release. Before removal, at least one minor release deprecates the interface while keeping it working and warning when it is used. The `MiniRacer` namespace follows mini_racer 0.22.1.

The environment variables `MINI_RACER_RS_SPIN_US`, `MINI_RACER_RS_CODE_CACHE`, and `MINI_RACER_RS_FAST_CALLS` receive one minor release of notice before removal. The deprecating release keeps the variable working and warns when it is set. Removal comes no earlier than the next minor release. This notice covers removal only. What these variables do to timing may change in any release.

## Platforms, Rubies and V8

Supported Rubies are CRuby 3.3, 3.4 and 4.0, plus TruffleRuby using mini_racer's vendored TruffleRuby backend. Supported Ruby versions follow CRuby's maintained releases.

Supported platforms are glibc and musl Linux, including Alpine, on x86_64 and aarch64, and macOS on arm64 and x86_64. Windows is not supported.

The extension uses V8 15.2 through the `v8` crate, pinned exactly at 152.2.0. Native gems contain one extension per supported Ruby. Source builds require Rust 1.91 or newer and libclang.
