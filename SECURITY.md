# Security

## Reporting a vulnerability

Report suspected vulnerabilities through GitHub private vulnerability reporting in this repository's Security tab. Do not open a public issue. The maintainer sends an initial response within 7 days of receiving a report.

## Supported versions

Security fixes land only in the latest release.

The gem follows semantic versioning, so a breaking change increments the major version. The V8 version changes only in a minor release, never a patch, and the release notes call out the change and any message wording it changes.

Supported Ruby implementations are CRuby 3.3, 3.4 and 4.0, plus TruffleRuby. Supported CRuby versions follow CRuby's maintained releases. Supported platforms are Linux with glibc or musl on x86_64 and aarch64, and macOS on arm64 and x86_64.

## Running untrusted JavaScript

JavaScript runs in plain V8 without Node.js APIs. It has no direct file system, network or process access. Ruby code reachable from JavaScript is exactly what your application exposes with `Context#attach`.

Each `MiniRacer::Context` runs V8 on its own native thread. `Context.new(timeout:)` bounds each `eval` and `call`. A watchdog thread terminates the script. If that thread cannot start, the operation raises `MiniRacer::InternalError` instead of running without a bound.

`Context.new(max_memory:)` sets a byte limit. After each V8 garbage collection, the context compares V8's used heap size with the limit. If usage is over it, the running script is terminated, or the next script if none is running. The affected call raises `MiniRacer::V8OutOfMemoryError` with the message `out of memory`, and the context remains usable. The heap can exceed the limit before collection detects it. `ensure_gc_after_idle:` takes milliseconds and runs one full collection after that idle period following a request. Zero disables it. V8's sandbox and pointer compression are disabled in the project's archives, and the repository boundary check rejects builds that enable those crate features.

Contexts in one process share a V8 code cache for sources of 1 MiB or more. A context, or a forked child that inherits the cache, can use timing to learn that a source with the same file name, length, and sampled content was evaluated before. For an exact match, it can also infer which functions ran during loading because the cache contains code compiled in that run. The cache cannot reveal values or objects from the earlier context. V8 stores only bytecode and function data. From the first evaluation, contexts also share one copy of such a source. After one evaluation, another context can use timing to distinguish a source with exactly the same file name and content from one matching only the length and sampled content. Setting `MINI_RACER_RS_CODE_CACHE=off` before the process creates its first context disables both the code cache and shared sources. Forked children retain the parent's setting.

## Supply chain

| Area | Control |
| --- | --- |
| V8 | The `v8` crate is pinned exactly at 152.2.0. Every supported target's archive and source binding has a pinned SHA-256 in `ext/mini_racer_rs/v8-archives.sha256`. Development, CI and source gem builds reject downloads that do not match. |
| Rust dependencies | `Cargo.lock` is committed and builds use `--locked`. CI runs cargo deny and cargo audit. Cargo deny checks advisories, licenses, bans, wildcard versions and sources, denying unknown registries and git sources. |
| Rust, Ruby, and npm lockfiles | OSV-Scanner scans eight Rust, Ruby, and npm lockfiles: `Cargo.lock`, the fuzz crate's `Cargo.lock`, `Gemfile.lock`, `package-lock.json`, `tools/reference/Gemfile.lock`, `release/sign/Gemfile.lock`, `test/consumers/js/package-lock.json`, and `test/consumers/transitive/Gemfile.lock`. On pull requests, newly added vulnerable versions fail the required `CI Pass` check. On `main` and weekly, any known vulnerability fails the scan. Accepted advisories and reasons are listed in `osv-scanner.toml`. Dependabot updates Cargo, Bundler, npm, and Alpine image digests with a 7-day cooldown, or 14 days for major versions. Alpine and signing bundle updates require manual merging. Other non-major updates enter the auto-merge queue after required checks pass. |
| GitHub Actions | Actions are pinned by commit SHA with version comments, and every workflow container image is pinned by digest. Alpine images use multi-architecture index digests. `scripts/check-action-pins.sh` rejects unpinned actions and container images. Workflows start with `permissions: {}` and grant each job only its required permissions. Dependabot updates Actions, but updates to `.github/workflows/release.yml` or files under `.github/actions/`, including the setup action used by the release workflow, require manual merge because that workflow builds, signs, and publishes the gems. CodeQL runs on pull requests. |
| Repository | Secret scanning, push protection and Dependabot security updates are enabled. CI must pass before squash merge. |
| Releases | The release workflow publishes through RubyGems trusted publishing with OIDC, without a stored API key. A GitHub environment requires owner approval and permits only `v*` tags. Each gem is signed with Sigstore by a job without the `rubygems` environment, using the checksum-locked bundle in `release/sign`. Each gem is pushed with its Sigstore attestation, which RubyGems.org displays as provenance starting with 1.2.0. If signing fails, publication continues without the attestation and logs a warning. GitHub releases attach every gem, each gem's Sigstore bundle, and `SHA256SUMS`. Build provenance attestations for every gem are stored in the repository's attestation store and can be verified with `gh attestation verify`. |
