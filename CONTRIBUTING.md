# Contributing

mini_racer_rs is a Rust implementation of the MiniRacer Ruby API and targets compatibility with mini_racer 0.22.2. It uses V8 15.2 through `v8` 152.2.0. [STABILITY.md](STABILITY.md#platforms-rubies-and-v8) lists the supported Rubies and platforms.

## Setup

Run:

```sh
bin/setup
```

This installs the pinned toolchains and tools, gems, npm packages, and V8 archive, then compiles the project. Source builds require Rust 1.91 or newer and libclang, supplied by the system or the `libclang` gem. Ruby 4.0 also requires libffi development headers (`libffi-dev` on Debian and Ubuntu or `libffi-devel` on Fedora) so `bundle install` can build the `fiddle` gem used by the fork tests.

Use these commands while developing:

```sh
scripts/gate.sh --quick
bundle exec rake compile
bundle exec rake test
scripts/gate.sh
scripts/gate.sh --full
```

`scripts/gate.sh --quick` runs static checks and clippy. The default gate adds Rust tests, compilation, export checks, and Ruby tests. `scripts/gate.sh --full` also runs planted-violation tests, MSRV checks, cargo deny, cargo audit, machete, rustdoc, and gem builds.

## Making a change

Create a branch named `<type>/<kebab-description>`. Commit messages follow Conventional Commits. The commit-msg hook runs commitlint through husky, which `npm ci` installs. Never bypass hooks.

The Rust crates have distinct responsibilities: `mini_racer_wire` implements the V8 wire format codec, `mini_racer_sync` contains concurrency protocol types checked with loom, `mini_racer_engine` contains V8 and runners, and `mini_racer_rs` is the extension crate. `scripts/check-boundaries.sh` enforces the dependency layering defined in `layers.toml`.

Do not use en dashes or em dashes in code, comments, documentation, or commit messages. `scripts/check-prose.sh` enforces this rule. Do not edit `vendor/mini_racer` or `vendor/execjs`; they are pinned upstream submodules.

## Matching mini_racer

The compatibility target is mini_racer's unmodified test suite at version 0.22.2. Its per-platform results are recorded in `test/upstream/mini_racer.status.toml`.

Every public surface identifier has a status in `compat.toml`. `COMPATIBILITY.md` is generated from that file. A behavior that differs from mini_racer 0.22.2 requires a test that fails first and a `compat.toml` entry stating the reason. Deliberate differences require both a reason and a test.

## Pull requests

Pull request titles follow Conventional Commits and are checked by `pr-title.yml`. Pull requests are squash-merged once the `CI Pass` check passes. No CLA is required.

See `AI-DISCLOSURE.md` for the project's use of AI tooling. Follow `CODE_OF_CONDUCT.md`, which contains the Contributor Covenant.
