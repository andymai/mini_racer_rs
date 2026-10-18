# Testing reference

## Ruby tests

`bundle exec rake test` compiles, then runs `test/ruby/**/*_test.rb` (`rakelib/test.rake`). `test/ruby/test_helper.rb` puts this gem's `lib/` first on the load path and turns on `GC.auto_compact` when `MINI_RACER_RS_AUTO_COMPACT` is set, as CI's GC Stress job does for a second `rake test`. `RubyProcess#run_ruby(code, env:, load_path:)` runs code in a fresh Ruby and returns stdout, stderr and the status; tests that set platform flags or need a clean process use it (`test/ruby/watchdog_test.rb`, `test/ruby/spent_termination_test.rb`).

## Fork cases

`test/ruby/fork_test.rb` defines `test_<what>` for every `test/ruby/fork/<what>_case.rb` and runs each case as its own Ruby process, since V8's platform mode is fixed for a process at its first context. It kills the case and every process it forked after 60 seconds, because a deadlock can hold the GVL and keep the case's own watchdog from running. A case passes by exiting 0.

A case starts with `require_relative "prelude"` (`test/ruby/fork/prelude.rb`), which loads this gem and defines:

- `child { ... }` forks, runs the block in the child, and fails the case unless the child exits 0 within five seconds;
- `reap(pid)` waits up to five seconds for a child, then kills it and raises;
- `raised(klass) { ... }` returns the message of the error the block raises, or nil;
- `skip_case(reason)` prints the reason and exits 77, which ForkTest reports as a skip; with `CI` set it exits 1 instead, so a CI leg missing a prerequisite fails rather than dropping the case.

The three cases that fork through Fiddle need `fiddle`. Ruby 4.0 ships without it, so the `Gemfile` installs it there (building it needs libffi's headers); Ruby 3.3 and 3.4 use their default gem. The case's top comment says what it proves and what mini_racer 0.22.1 does.

## Loom models

`crates/mini_racer_sync/tests/loom.rs` is `#![cfg(loom)]`. Protocol code names only `mini_racer_sync::primitives`, which re-exports loom's types under `--cfg loom` and std's otherwise, so a model checks the code the engine runs; a `std::sync` type in protocol code is invisible to loom. Each model opens with a comment stating what it proves (most are numbered, `// Model <n>:`), and its name states that property. Small models call `loom::model`; larger ones call `bounded(<preemptions>, || ...)`, which sets loom's preemption bound so they finish in seconds.

## Differential corpus

`test/diff/corpus/*.json` files are arrays of cases that every values run of `test/diff/run.rb` includes (not `--explore`, not `--set consumers`). A hand-written case:

```json
{ "id": "curated-<name>", "kind": "program", "steps": [{ "op": "eval", "source": "new Date(NaN)" }] }
```

Steps (`test/diff/worker.rb`): `eval` with `source`; `call` with `path` and `args` in the canonical form of `test/diff/canonical.rb`; `attach` with `name` and `callable` (`echo` or `values`); `execjs`. A step with `"setup": true` is compared only on whether it raised. After adding a case, `bundle exec ruby test/diff/run.rb --write-ledger` records every case's class in `test/diff/candidate.status.toml` (`same`, `accepted:<divergence>`, a new divergence as `xfail:M1`, a candidate crash as `crash:M1`); review that diff, then run with `--twin --strict`. `--shrink` writes minimized divergences as `test/diff/corpus/min-<id>.json`. Every run needs the real gem: `cd tools/reference && BUNDLE_PATH=vendor/bundle bundle install`.

## Consumer workloads

`DiffConsumers::WORKLOADS` in `test/consumers/workloads.rb` maps a name to steps; `load_bundle("<bundle>")` evaluates `tmp/consumers/bundles/<bundle>.js`, which `npm ci && npm run build` in `test/consumers/js` builds from `test/consumers/js/entries/`. Cases run as `consumer-<name>`. A step that fails on the real gem aborts the run, since that means the workload or its bundle is broken. Record outcomes with `bundle exec ruby test/diff/run.rb --set consumers --write-ledger` into `test/consumers/candidate.status.toml`.

## Stress scripts

Each script runs alone with `bundle exec ruby -Ilib test/stress/<script>.rb`, takes the platform mode as an argument where it matters (fixed per process), prints what it measured, and exits nonzero with a message saying what it saw.

| Script | Fails when | Where it runs |
|---|---|---|
| `test/stress/round_trips.rb single\|multi [count] [warmup] [max_growth]` | RSS grows over 5% after the warm-up | Nightly and PRs labeled `stress`: 10,000,000 evals per mode |
| `test/stress/churn.rb single\|multi [threads] [seconds] [max_bytes_per_context]` | RSS grows over 256 bytes per context plus 32 MB | Nightly and `stress`: 8 threads for 600 s per mode |
| `test/stress/fork_loop.rb [quiet] [busy]` | a child fails or hangs past 3 s | every CI Ruby leg |
| `test/stress/fork_busy.rb [forks] [threads]` | a child of a multi-threaded parent, forked while other threads evaluate, fails or hangs past 3 s | every CI Ruby leg: 300 forks, 4 threads |
| `test/stress/fork_child_memory.rb [evals] [margin_mib]` | a child of a multi-threaded parent grows more than the margin (8 MiB) past its parent over the same allocating loop in a new context | every CI Ruby leg on Linux: 20,000 evals each |
| `test/stress/spawn_churn.rb single\|multi [spawns]` | a spawn hangs or takes over a second | every CI Ruby leg, both modes |
| `test/stress/large_load.rb single\|multi [contexts] [max_mib]` | a context loaded with a generated 3 MiB script costs over 16 MiB of RSS | every CI Ruby leg on Linux, both modes |
| `test/stress/madvise_fixture.rb clean\|crash` | the child's outcome is not the expected one | `scripts/madvise-fixture.sh`, CI's ubuntu-24.04 Ruby 3.4 leg |

The churn bound is not zero because rusty_v8 leaks a 64-byte cell per isolate (denoland/rusty_v8#2075). The nightly `stress` job runs on ubuntu-24.04 and macos-15, one step per run with `if: ${{ !cancelled() }}`, so one failure does not hide the others; a new long-running script gets a step there.

## Observing the real gem

To see what mini_racer 0.22.1 does, for a deviation's reason or a test's comment, install the reference bundle once (`cd tools/reference && BUNDLE_PATH=vendor/bundle bundle install`), then run a snippet against it from the repository root:

```bash
BUNDLE_GEMFILE="$PWD/tools/reference/Gemfile" BUNDLE_PATH="$PWD/tools/reference/vendor/bundle" \
  bundle exec ruby -e 'require "mini_racer"; p MiniRacer::Context.new.eval("Object(1n)")'
```

`bundle exec ruby test/upstream/run.rb --reference` runs upstream's suite on the real gem the same way.
