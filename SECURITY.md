# Security

## Reporting a vulnerability

Report suspected vulnerabilities through GitHub private vulnerability reporting in this repository's Security tab. Do not open a public issue. The maintainer sends an initial response within 7 days of receiving a report.

## Supported versions

mini_racer_rs is pre-release and is not published to RubyGems.org. Security fixes land only in the latest release.

The gem uses semantic versioning and starts below 1.0. Before 1.0, breaking changes increment the minor version. The V8 version changes only in a minor release, never a patch, and the release notes call out the change and any message wording it changes.

Supported Ruby implementations are CRuby 3.3, 3.4 and 4.0, plus TruffleRuby. Supported CRuby versions follow CRuby's maintained releases. Supported platforms are Linux with glibc on x86_64 and aarch64, and macOS on arm64 and x86_64.

## Running untrusted JavaScript

JavaScript runs in plain V8 without Node.js APIs. It has no direct file system, network or process access. Ruby code reachable from JavaScript is exactly what your application exposes with `Context#attach`.

Each `MiniRacer::Context` runs V8 on its own native thread. `Context.new(timeout:)` bounds each `eval` and `call`. A watchdog thread terminates the script. If that thread cannot start, the operation raises `MiniRacer::InternalError` instead of running without a bound.

`Context.new(max_memory:)` sets a byte limit. After each V8 garbage collection, the context compares V8's used heap size with the limit. If usage is over it, the running script is terminated, or the next script if none is running. The affected call raises `MiniRacer::V8OutOfMemoryError` with the message `out of memory`, and the context remains usable. The heap can exceed the limit before collection detects it. `ensure_gc_after_idle:` takes milliseconds and runs one full collection after that idle period following a request. Zero disables it. V8's sandbox and pointer compression are disabled in the project's archives, and the repository boundary check rejects builds that enable those crate features.

Contexts in one process share a V8 code cache for sources of 1 MiB or more, so a context, or a forked child that inherits the cache, can use timing to learn that a source with the same file name, length, and sampled content was evaluated before. For an exact match, they can also infer which functions ran during loading, because the cache contains code compiled in that run. They cannot recover values or objects from the earlier context: V8 stores bytecode and function data only. Set `MINI_RACER_RS_CODE_CACHE=off` before the process creates its first context to disable it. Forked children retain the parent's setting.

## Supply chain

| Area | Control |
| --- | --- |
| V8 | The `v8` crate is pinned exactly at 152.2.0. Every supported target's archive and source binding has a pinned SHA-256 in `ext/mini_racer_rs/v8-archives.sha256`. Development, CI and source gem builds reject downloads that do not match. |
| Rust dependencies | `Cargo.lock` is committed and builds use `--locked`. CI runs cargo deny and cargo audit. Cargo deny checks advisories, licenses, bans, wildcard versions and sources, denying unknown registries and git sources. |
| Ruby and npm lockfiles | OSV-Scanner scans seven Rust, Ruby, and npm lockfiles, including tooling and test lockfiles. On pull requests, it compares the base branch with the pull request, and newly added vulnerable versions fail the required `CI Pass` check. On `main` and weekly, any known vulnerability fails the scan. Accepted advisories and their reasons are listed in `osv-scanner.toml`. Dependabot updates Cargo, Bundler, and npm dependencies with a 7-day cooldown, or 14 days for major versions. |
| GitHub Actions | Actions are pinned by commit SHA with version comments. `scripts/check-action-pins.sh` verifies the pins. Workflows start with `permissions: {}` and grant each job only its required permissions. Dependabot updates Actions, and CodeQL runs on pull requests. |
| Repository | Secret scanning, push protection and Dependabot security updates are enabled. CI must pass before squash merge. |
| Releases | The release workflow publishes through RubyGems trusted publishing (OIDC), without a stored API key. A GitHub environment requires owner approval and permits only `v*` tags. Releases attach `SHA256SUMS` and build provenance attestations for every gem. |
