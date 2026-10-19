---
name: testing
description: Use when writing, placing or running a test in mini_racer_rs; when a fix or a deviation from mini_racer 0.22.1 needs a failing-first test; when adding a wire golden corpus entry, a loom model, a fork case, a differential corpus case, a consumer workload or a stress script; when a change moves an upstream suite result; or when choosing which commands to run before a PR.
---

# Testing

A change gets a test at the lowest layer that can see it, and the suites above it stay green: mini_racer 0.22.1's own tests run unmodified against the ledgers, and the differential run compares outcomes with the real gem. The commands for each changed path are in `CLAUDE.md` under "Which tests to run".

## Where a test goes

| It checks | Put it in | Run one |
|---|---|---|
| One module's logic, no V8 or Ruby | `#[cfg(test)] mod tests` in that file | `cargo nextest run -p <crate> <name>` |
| Ruby-free extension rules | `#[cfg(test)]` in `ext/mini_racer_rs/src/options.rs` or `ext/mini_racer_rs/src/decode.rs` | `cargo test -p mini_racer_rs --lib` (its binary links libruby; CI runs it in the Ruby jobs) |
| Decoding built bytes, round trips | `crates/mini_racer_wire/tests/edges.rs`, `crates/mini_racer_wire/tests/roundtrip.rs` | `cargo nextest run -p mini_racer_wire` |
| What V8 writes | a line in `crates/mini_racer_wire/tests/golden/corpus.tsv` | Golden corpus below |
| Anything that needs V8 | `crates/mini_racer_engine/tests/<area>.rs`, helpers in `crates/mini_racer_engine/tests/common/mod.rs` | `cargo nextest run -p mini_racer_engine` |
| A V8 flag or platform mode | the one test in `crates/mini_racer_engine/tests/flags.rs`, or a test file of its own | same |
| A protocol interleaving | a model in `crates/mini_racer_sync/tests/loom.rs` | the loom command in `CLAUDE.md`, then `-- <model>` |
| The Ruby API | `test/ruby/<area>_test.rb` | `bundle exec ruby -Itest/ruby test/ruby/<area>_test.rb -n <test>` |
| Forking with contexts | `test/ruby/fork/<what>_case.rb` | `bundle exec ruby test/ruby/fork/<what>_case.rb; echo $?` |
| Little stack left on a fiber or a thread | `test/ruby/stack/<what>_case.rb` | `bundle exec ruby -Itest/ruby test/ruby/stack_test.rb -n test_<what>` (StackTest sets Ruby's smallest stacks) |
| Values or programs against the real gem | a case in `test/diff/corpus/curated.json` | `bundle exec ruby test/diff/run.rb --strict` |
| A real library | `DiffConsumers::WORKLOADS` in `test/consumers/workloads.rb` | `bundle exec ruby test/diff/run.rb --set consumers --strict` |
| Memory or lifecycle over minutes | a script in `test/stress/` | reference.md |
| A README example | a fenced `ruby check` block in `README.md`; every `# =>` value is asserted | `bundle exec ruby -Itest/ruby test/ruby/readme_test.rb` |

V8's flags and platform mode freeze at first use, once per process. `cargo test`, which `scripts/sanitizers.sh` uses, runs a binary's tests in one process, so a flag-dependent assertion joins `flags_apply_before_initialization` in `crates/mini_racer_engine/tests/flags.rs` or gets its own test file, as `crates/mini_racer_engine/tests/teardown_single_threaded.rs` does. Ruby code that sets flags runs through `run_ruby` (`include RubyProcess`, from `test/ruby/test_helper.rb`), which starts a fresh Ruby.

`clippy.toml` allows `expect`, `unwrap`, `panic` and printing inside `#[test]` functions and `#[cfg(test)]` modules. An integration-test helper outside a `#[test]` that calls `expect` or panics needs a file-level `#![allow(clippy::expect_used, clippy::panic, reason = "...")]` naming the lints it uses, as `crates/mini_racer_engine/tests/common/mod.rs` has.

## Golden corpus

1. Add `<name>`, a tab, then the JavaScript, as a line of `crates/mini_racer_wire/tests/golden/corpus.tsv`.
2. `cargo run -p mini_racer_engine --example gen_wire_golden` writes `<name>.bin`, the bytes V8 serializes.
3. `UPDATE_GOLDEN=1 cargo test -p mini_racer_wire --test golden` writes `<name>.txt`, their decoding.
4. Read every new or changed `.txt` before committing: it is the reviewed expectation. CI's Rust Tests job regenerates the `.bin` files and fails on any drift.

## Upstream ledgers

`vendor/mini_racer` and `vendor/execjs` are never edited. A change that moves an upstream result updates its entry in `test/upstream/mini_racer.status.toml` or `test/upstream/execjs.status.toml` in the same PR, with its reason: `pass` when an `xfail` or `crash` test starts passing (strict mode fails until it does), `upstream-fail` with a `cause`, `deviation:<id>` naming a `compat.toml` deviation, `crash:<M>` with `timeout_s`. Statuses and regeneration: `.claude/skills/parity-suite`.

An upstream test that moves values between JavaScript and Ruby belongs in `test/upstream/gc_stress.txt`; `bundle exec ruby test/upstream/run.rb --gc-stress --strict` runs the listed tests that pass on this leg under `GC.stress` with a compaction check after each. This gem's own conversion tests under compaction live in `test/ruby/conversion_gc_test.rb`.

## Failing first

A fix, and any behavior that differs from mini_racer 0.22.1, lands with a test that failed before the change (the PR template asks).

1. Write the test for the behavior this gem will have. A deviation test opens with a comment naming its compat id and what mini_racer 0.22.1 does, as in `test/ruby/callback_deviation_test.rb`.
2. Run it on the code before the change: commit the test alone first and run it on that commit. The stash is shared by every worktree, so do not park the change there. After touching Rust, `bundle exec rake compile` before every Ruby run, or the previous extension keeps loading. It must fail on its assertion, not on a load or name error.
3. Restore the change, compile, run again: it passes.
4. For a deviation, observe the real gem (reference.md), then add `[ids."behavior:<kebab>@ruby"]` to `compat.toml` with `status = "deviation"` and a `reason` stating both behaviors; the `approved` date is Andy's to add. The reason renders into `COMPATIBILITY.md`, so it is public prose and goes through `codex-write`. Run `bundle exec rake compat:render`, then `bundle exec rake compat:check`. A differential divergence it explains gets an entry in `test/diff/divergences.toml` naming that id.

Ruby test, fork case, corpus, consumer and stress details: [reference.md](reference.md).
