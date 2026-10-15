# PR Workflow reference

## CI jobs (`.github/workflows/ci.yml`)

`CI Pass` needs every job below to succeed or be skipped. Docs-only PRs (Markdown, `LICENSE`, issue templates, funding) skip the build jobs.

| Job | Runs |
|---|---|
| Changes | Decides whether the PR touches code |
| Format | `cargo fmt --all -- --check` |
| TOML Format | `taplo fmt --check` |
| RuboCop | `bundle exec rubocop` |
| Unused Dependencies | `cargo machete` |
| Cargo Deny | `cargo deny check` |
| Security Audit | `cargo audit` |
| Boundaries | `scripts/check-boundaries.sh`, `scripts/test-boundaries.sh`, and the fence, guard, GC root and static checks |
| Repo Checks | version, action pin, platform, prose, doc path and Claude hook checks |
| MSRV | `cargo check` on the toolchain in `tools/versions.toml` |
| Rustdoc | `cargo doc` with `-D warnings` |
| Clippy | `cargo clippy --workspace --all-targets -- -D warnings` |
| Rust Tests | nextest and doctests, without the extension crate |
| Gem Contents | Builds both gemspecs, checks the file list, resolves the unpacked source gem with `--locked` |
| Ruby (ubuntu-24.04, macos-15) | `rake compile`, `scripts/check-exports.sh`, `rake test`, and a source gem install into an empty `GEM_HOME` |

`.github/workflows/pr-title.yml` lints the PR title with commitlint. It runs from `main` (`pull_request_target`), so a change to it takes effect after merge.

## Setup action (`.github/actions/setup/action.yml`)

Installs the toolchain from `rust-toolchain.toml`, restores and verifies the V8 archive, caches `target/`, and optionally installs Ruby (with the bundler cache) and tools at the versions pinned in `tools/versions.toml`.

## PR body brief

The `codex-write` brief lists every fact with its source: what the diff adds, how it was verified (commands and counts), deviations from the plan with reasons, and what only CI can confirm. Bodies state observable facts, follow any repository template, use LF line endings, and are never hard-wrapped.
