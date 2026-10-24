---
name: debugging
description: Use when triaging an ASan, LSan or TSan report (scripts/sanitizers.sh or the nightly Sanitizers job), a failing loom model, a fuzz crash, an upstream test that times out or crashes or a suite failure, a differential divergence or a worker dying with signal 6, a failing or hanging fork case, a "Scheduled <workflow> run failed" issue, a Rust edit that seems to have no effect (a stale extension), or a link that fails with "could not find native static library rusty_v8".
---

# Debugging

Every harness here writes its evidence to a known place. Read that first, reproduce the narrowest case locally, then fix it and keep the reproduction as a test (`.claude/skills/testing`). Ledger statuses for upstream results are in `.claude/skills/parity-suite`.

## Quick reference

| Symptom | Evidence | Reproduce |
|---|---|---|
| ASan or LSan report | the output of `scripts/sanitizers.sh`; the nightly Sanitizers job log (it uploads nothing) | one test binary under the same flags (reference.md) |
| TSan report | same | the TSan line of `scripts/sanitizers.sh` |
| Loom model fails | the model's panic message | the loom command in `CLAUDE.md`, then `-- <model>` |
| Fuzz crash | `crates/mini_racer_wire/fuzz/artifacts/<target>/`; CI artifact `fuzz-artifacts` | `cargo fuzz run <target> <input>` on the pinned nightly |
| Upstream `timeout` | `tmp/upstream/<test id>.timeout.txt` (Ruby threads, then native stacks) | `bundle exec ruby test/upstream/run.rb -n <test> --seed <seed>` |
| Upstream `crash` or suite failure | the attempt's log in `tmp/upstream/` | same |
| Differential `diverged` or `crashed` | `tmp/diff/values-<seed>.jsonl`, `tmp/diff/<role>-worker.log` | `bundle exec ruby test/diff/run.rb --seed <n> --shrink` |
| Fork case fails or hangs | ForkTest's message: status, stdout, stderr | `bundle exec ruby test/ruby/fork/<what>_case.rb; echo $?` |
| Scheduled run issue | the issue: run link, commit, failed jobs | `gh run view <run-id> --log-failed` |
| A Rust change has no effect | which extension `$LOADED_FEATURES` lists | `bundle exec rake compile` |
| `Fatal JavaScript out of memory` with a heap limit set, or a `MiniRacerRs::HeapLimitError` a script did not earn | V8's line on stderr (`Reached heap limit`, `CALL_AND_RETRY_LAST`, `Ineffective mark-compacts near heap limit`); `.claude/skills/engine-internals/heap-limit.md` | the script alone through `capped` in `test/ruby/heap_limit_test.rb`, or `crates/mini_racer_engine/tests/heap_limit.rs` |
| `could not find native static library rusty_v8` | the v8 crate rebuilt without rerunning its build script, so `gn_out` is gone | reference.md, "Missing V8 static library" |

## Rules

- Reproduce before theorizing. Loom models, fuzz inputs and the differential generator are deterministic: the same model, input or `--seed` fails the same way. The upstream suite's `--seed` fixes only the test order.
- A hang is debugged from stacks, not from guesses: the upstream watchdog captures them, and `eu-stack -p <pid>` (Linux) or `sample <pid> 1` (macOS) shows a live process's native stacks.
- `["crash","signal 6"]` on the reference side is mini_racer aborting; it is an outcome, not a harness failure.
- Never silence a check to get green: no new LSan suppression without the upstream leak it matches, no ledger status change without the result it records, no `--no-verify`.

## Stale extension

`lib/mini_racer_rs.rb` loads `lib/mini_racer_rs/<ruby minor>/mini_racer_rs.<dlext>` when it exists (an extension copied from a native gem, as the release-flow skill does to run a suite on a released build), and `lib/mini_racer_rs/mini_racer_rs.so` otherwise. The plain build keeps loading after Rust edits until `rake compile` rebuilds it; a per-minor copy wins over every build until it is deleted. `NATIVE_VERSION` catches only a version mismatch. The symptom is a fix that changes nothing, or a method in the source that Ruby calls undefined.

```bash
bundle exec ruby -Ilib -e 'require "mini_racer"; puts $LOADED_FEATURES.grep(/mini_racer_rs\.(so|bundle)\z/)'
ls lib/mini_racer_rs/          # a directory named for a Ruby minor is a copied extension: delete it
bundle exec rake compile
```

A `CI=1` release build from benchmarking also stays until the next `rake compile`. Switching profiles (`CI=1 rake compile` after a dev build, or back) can reinstall the other profile's copy staged under `tmp/<platform>/mini_racer_rs/<ruby version>/`, which make finds newer than cargo's output; delete it and `lib/mini_racer_rs/mini_racer_rs.so` first, and check with `cmp` against `target/release/libmini_racer_rs.so`.

Recipes per symptom: [reference.md](reference.md).
