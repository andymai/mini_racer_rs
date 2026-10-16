---
name: v8-bump
description: Use when moving the pinned v8 crate (rusty_v8) to a new version, when the weekly Extended workflow's newest-v8 job reports a newer release or fails, or when a change touches the V8 archive digests, ICU data, the export allowlists, the wire goldens or V8-worded parity results.
---

# V8 Bump

`v8` is pinned exactly in `Cargo.toml` (`workspace.dependencies.v8`) and Dependabot ignores it, as it ignores `deno_core_icudata`: the engine loads ICU data through `v8::icu::set_common_data_78` in `crates/mini_racer_engine/src/process.rs`, so the ICU data crate moves with V8's ICU version. A bump changes the archive digests, the export allowlists, the wire goldens and the third-party notices together, in one PR.

## Procedure

1. Pick the version. The Extended workflow's "Newest v8 crate" job writes the pinned and newest versions to its summary every week and shows what breaks; read rusty_v8's release notes for the V8 version it carries.
2. Set the pin in `Cargo.toml` and run `cargo update -p v8`. If V8's ICU major changed, move `deno_core_icudata` to the matching release and rename the `set_common_data_<major>` call.
3. `scripts/update-v8-archives.sh` downloads every target's archive and source binding and rewrites `ext/mini_racer_rs/v8-archives.sha256`; then `. scripts/fetch-v8-archive.sh`.
4. Build and fix API changes: `scripts/gate.sh --quick`, then `cargo nextest run --workspace --exclude mini_racer_rs`.
5. Exports: `scripts/check-exports.sh --write` on Linux. On macOS, CI's `ruby` job prints the built list in its "Built exports" group when the check fails; copy it into `ext/mini_racer_rs/exports.allow.macos`.
6. Wire goldens: `cargo run -p mini_racer_engine --example gen_wire_golden`, then `UPDATE_GOLDEN=1 cargo test -p mini_racer_wire --test golden`, and read every changed `.txt`. A new wire version or tag is a `mini_racer_wire` change first.
7. Notices: `scripts/update-notices.sh`. It fails when an archive links source that `licenses/v8/components.tsv` does not list; add the component and its license there.
8. Ruby: `test/ruby/loader_test.rb` checks the V8 major and minor in `MiniRacerRs::V8_VERSION`; update it, then `bundle exec rake test`.
9. Parity: open the PR with the `ledger-generate` label and merge the ledgers (parity-suite skill). A message that changed only with V8's wording becomes `deviation:behavior:v8-version@ruby`, with its evidence.
10. `scripts/check-boundaries.sh` still holds: V8's sandbox and pointer compression features stay off.

After the PR merges, a dry run of the Release workflow on `main` proves the new archives build, link and load on every platform and Ruby.
