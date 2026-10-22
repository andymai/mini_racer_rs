# PR Workflow reference

## CI jobs (`.github/workflows/ci.yml`)

`CI Pass` needs every job below to succeed or be skipped. Docs-only PRs (Markdown, `LICENSE`, issue templates, funding) skip the build jobs.

| Job | Runs |
|---|---|
| Changes | Decides whether the PR touches code |
| Format | `cargo fmt --all -- --check`, and the same for the fuzz crate |
| TOML Format | `taplo fmt --check` |
| RuboCop | `bundle exec rubocop` |
| Unused Dependencies | `cargo machete` |
| Cargo Deny | `scripts/check-deny.sh`: the supported targets, every target (the only pass that sees `cfg(loom)` crates), and the fuzz crate |
| Security Audit | `cargo audit` on the root and fuzz lockfiles |
| Third-Party Notices | `scripts/update-notices.sh --check`, on every PR: `THIRD_PARTY_NOTICES.md` matches `cargo about`, `licenses/rust` and `licenses/v8`, and none of the six V8 archives links source `licenses/v8/components.tsv` misses |
| Boundaries | `scripts/check-boundaries.sh`, `scripts/test-boundaries.sh`, and the fence, guard, GC root and static checks |
| Repo Checks | version, action and container image pin, scheduled-workflow watch, platform, prose, doc path and Claude hook checks |
| MSRV | `cargo check` on the toolchain in `tools/versions.toml` |
| Rustdoc | `cargo doc` with `-D warnings` |
| Clippy | `cargo clippy --workspace --all-targets -- -D warnings` |
| Rust Tests | nextest and doctests, without the extension crate, then regenerates the wire golden bytes from V8 and fails on any difference |
| Loom | the loom models in `crates/mini_racer_sync/tests/loom.rs`, under `--cfg loom` |
| Fuzz Smoke | `scripts/fuzz-smoke.sh 60`: every fuzz target for 60 s on the pinned nightly; crash inputs are uploaded on failure |
| Gem Contents | Builds both gemspecs, checks the file list, resolves the unpacked source gem with `--locked` |
| Ruby (3.3, 3.4 and 4.0 on ubuntu-24.04 and macos-15; 3.4 on ubuntu-24.04-arm and macos-15-intel) | `rake compile`, `scripts/check-exports.sh`, `rake test`, the upstream suite in strict mode, upstream's scripts, and a source gem install into an empty `GEM_HOME` |
| Alpine (Ruby 3.4, x86_64 on ubuntu-24.04 and aarch64 on ubuntu-24.04-arm) | `scripts/alpine.sh test` in the digest-pinned `ruby:3.4-alpine` from `release/alpine/Dockerfile`: `rake compile`, `scripts/check-exports.sh`, the Ruby tests as the checkout's owner, both upstream suites in strict mode, and `scripts/madvise-fixture.sh` |
| TruffleRuby | the harness guard tests and the upstream suite in strict mode against the vendored backend |
| OSV Scan (new findings), PRs only | OSV on the seven lockfiles at the base and the head; fails on a finding the PR adds or a scan that cannot finish |
| GC Stress (Ruby 3.4, ubuntu-24.04) | the upstream tests in `test/upstream/gc_stress.txt` that pass, each under `GC.stress` with compaction, then `rake test` with `GC.auto_compact` |

OSV scans all seven lockfiles: on PRs the `osv` job in `.github/workflows/ci.yml` fails on findings the PR adds, and `.github/workflows/osv-scan.yml` fails `main` and the weekly run on any finding; accepted advisories go in `osv-scanner.toml`. `.github/workflows/dependabot-auto-merge.yml` queues Dependabot's patch and minor updates for auto-merge once the required checks pass, through the release-kun App when its secrets are also set as Dependabot secrets, except `release/sign` and the Alpine image digests in `release/alpine/Dockerfile`, which the release runs; `.github/dependabot.yml` keeps `v8`, `deno_core_icudata`, `rb-sys`/`rb_sys` and the reference gem's `mini_racer`/`libv8-node` for deliberate bumps.

`.github/workflows/nightly.yml` runs `scripts/sanitizers.sh` every night, on demand, and on PRs carrying the `sanitizers` label: AddressSanitizer on the engine's teardown tests with leak detection (rusty_v8's deliberate per-isolate leak is suppressed in `.github/sanitizers/lsan.supp`), on every engine and sync test for memory errors, and ThreadSanitizer on the sync crate. It is not part of `CI Pass`.

`.github/workflows/pr-title.yml` lints the PR title with commitlint. It runs from `main` (`pull_request_target`), so a change to it takes effect after merge.

## Setup action (`.github/actions/setup/action.yml`)

Installs the toolchain from `rust-toolchain.toml`, restores and verifies the V8 archive, caches `target/`, and optionally installs Ruby (with the bundler cache) and tools at the versions pinned in `tools/versions.toml`.

## PR body brief

The `codex-write` brief lists every fact with its source: what the diff adds, how it was verified (commands and counts), deviations from the plan with reasons, and what only CI can confirm. Bodies state observable facts, follow any repository template, use LF line endings, and are never hard-wrapped.
