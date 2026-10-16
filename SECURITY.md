# Security

## Reporting a vulnerability

Report suspected vulnerabilities through GitHub private vulnerability reporting in this repository's Security tab. Do not open a public issue.

## Supported versions

mini_racer_rs is pre-release and is not published to RubyGems.org. Security fixes land only in the latest release.

The gem uses semantic versioning and starts below 1.0. Before 1.0, breaking changes increment the minor version. The V8 version changes only in a minor release, never a patch, and the release notes call out the change and any message wording it changes.

Supported Ruby implementations are CRuby 3.3, 3.4 and 4.0, plus TruffleRuby. Supported CRuby versions follow CRuby's maintained releases. Supported platforms are Linux with glibc on x86_64 and aarch64, and macOS on arm64 and x86_64.

## Running untrusted JavaScript

JavaScript runs in plain V8 without Node.js APIs. It has no direct file system, network or process access. Ruby code reachable from JavaScript is exactly what your application exposes with `Context#attach`.

Each `MiniRacer::Context` runs V8 on its own native thread. `Context.new(timeout:)` bounds each `eval` and `call`. A watchdog thread terminates the script. If that thread cannot start, the operation raises `MiniRacer::InternalError` instead of running without a bound.

`Context.new(max_memory:)` and `ensure_gc_after_idle:` raise `NotImplementedError`. V8's sandbox and pointer compression are disabled in the archives used by this project. The repository boundary check rejects builds that enable those crate features.

## Supply chain

| Area | Control |
| --- | --- |
| V8 | The `v8` crate is pinned exactly at 152.2.0. Every supported target's archive and source binding has a pinned SHA-256 in `ext/mini_racer_rs/v8-archives.sha256`. Development, CI and source gem builds reject downloads that do not match. |
| Rust dependencies | `Cargo.lock` is committed and builds use `--locked`. CI runs cargo deny and cargo audit. Cargo deny checks advisories, licenses, bans, wildcard versions and sources, denying unknown registries and git sources. |
| Ruby and npm lockfiles | OSV-Scanner scans seven Rust, Ruby and npm lockfiles, including tooling and test lockfiles. Findings are reported on pull requests and fail checks on `main` and weekly runs. Dependabot updates cargo, Bundler and npm dependencies with a 7-day cooldown, or 14 days for major versions. |
| GitHub Actions | Actions are pinned by commit SHA with version comments. `scripts/check-action-pins.sh` verifies the pins. Workflows start with `permissions: {}` and grant each job only its required permissions. Dependabot updates Actions, and CodeQL runs on pull requests. |
| Repository | Secret scanning, push protection and Dependabot security updates are enabled. CI must pass before squash merge. |
| Releases | The release workflow publishes through RubyGems trusted publishing (OIDC), without a stored API key. A GitHub environment requires owner approval and permits only `v*` tags. Releases attach `SHA256SUMS` and build provenance attestations for every gem. |
