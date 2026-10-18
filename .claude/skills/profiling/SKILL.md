---
name: profiling
description: Use when making or measuring a performance change in mini_racer_rs, when taking a bench/run.rb baseline, choosing metrics for round trips, payloads or workloads, reading noisy, unsupported or different-output rows, building the release extension for numbers, pinning CPUs, timing the stress scripts, profiling one benchmark process, or before quoting a speedup.
---

# Profiling

`bench/run.rb` measures this gem against the real mini_racer 0.22.1 on the same machine and reports medians and ratios. The numbers are informational: CI's Ruby job runs `--quick` as a smoke test, `.github/workflows/bench.yml` runs the full set weekly on a shared hosted runner, and nothing fails on either. A CI perf gate lands after the first release. Measure before changing anything, on a release build, and quote a number only from a full pinned run, with its row's note beside it.

## Setup

1. The real gem: `cd tools/reference && BUNDLE_PATH=vendor/bundle bundle install`.
2. The workload bundles: `npm ci && npm run build` in `test/consumers/js` (into `tmp/consumers/bundles`).
3. A release build: `CI=1 bundle exec rake compile`. `rakelib/compile.rake` builds the dev profile unless `CI` is set, and unoptimized numbers say nothing about a release. The release profile keeps line tables (`debug = "line-tables-only"` in `Cargo.toml`), so profiles have symbols and lines. The release build stays loaded until the next plain `bundle exec rake compile`.
4. The machine: run.rb prints the CPU governor it reads from `/sys/devices/system/cpu/cpu0/cpufreq/scaling_governor`; `BENCHMARKS.md` uses `performance`, with both gems pinned to the same eight cores (`--cpus 0-7`, through `taskset`).

## Quick reference

| Task | Command |
|---|---|
| Everything, 5 runs, pinned | `bundle exec ruby bench/run.rb --cpus 0-7` |
| More runs for a noisy row | add `--runs <n>` |
| Smoke run (one short run of each) | add `--quick` |
| Round trips, startup and payload MB/s only | add `--no-serde --no-workloads --no-memory` |
| Conversion per value type (mini_racer's `benchmark/serde`) | add `--no-micro --no-workloads --no-memory` |
| The library workloads only (Babel, Terser, Autoprefixer, React, MUI, Handlebars) | add `--no-micro --no-serde --no-memory` |
| Resident memory only (idle, loaded and churned contexts) | add `--no-micro --no-serde --no-workloads` |
| A server rendering bundle kept outside the repository | set `MINI_RACER_RS_BENCH_SSR_BUNDLE`, and `MINI_RACER_RS_BENCH_SSR_PRELUDE` and `MINI_RACER_RS_BENCH_SSR_CALL` as `bench/external.rb` describes; its rows record no path, name or error message |
| Results | `tmp/bench/<UTC timestamp>.json`: `summary`, `serde`, `workloads`, `external`, `memory` (medians, ratio, note per row) and every raw run |

## How a run works

Each run starts the reference, then the candidate, as separate `bundle exec` processes started the way `test/diff/workers.rb` starts the differential run's workers: `bench/measure.rb` for the per-call metrics, `bench/serde.rb` for upstream's serde suite, `bench/workloads.rb <name>` once per workload and `bench/memory.rb <scenario>` once per memory scenario, so what one leaves behind cannot change another. Tables report the median across runs; the ratio is how many times faster this gem is. A failure on the reference side aborts the run, except that a reference process killed by a signal is run up to three times in all, since mini_racer 0.22.1 has aborted on its own corrupted heap during `bench/memory.rb` (`corrupted double-linked list`); a metric the candidate cannot run is a row noted `unsupported`. Other notes: `noisy` (one gem's runs spread more than 10%, which run.rb reads as something else using the machine; rerun before trusting the ratio), `different output` (the payload or workload digests differ, which is a correctness bug, not a number), `under 1.0` (idle CPU below its floor, no ratio), `reference unsupported` (the real gem could not run it, no ratio).

## What to measure first

| Change touches | Watch first | Then |
|---|---|---|
| Dispatch, the channel, spinning and parking, GVL release, the watchdog | eval, call and callback round trip p50 and p99, call with timeout | `bundle exec ruby -Ilib test/stress/round_trips.rb multi` (microseconds per eval, and RSS) |
| Wire decode or write, encode, text transcoding | JS to Ruby and Ruby to JS MB/s | the serde rows for the value type that changed |
| Context creation, dispose, snapshots, isolate teardown | startup p50, Babel snapshot startup and size, and RSS per context from one Babel snapshot (`babel_snapshot_contexts`) | `bundle exec ruby -Ilib test/stress/churn.rb multi 8 60` (contexts churned, bytes per context) |
| What a context keeps allocated: buffers, isolate setup, what V8 or malloc holds after a load | the memory rows (each scenario in its own process; per-context rows leave out a first context) | the same rows with `--runs 3` or more, since resident memory moves with the allocator |
| Anything on the hot path | the round trips above | the workloads: they spend most of their time inside V8, so they confirm real use does not regress rather than show small wins |
| Shared sources (`process::sources`, `sys::external_string`) | RSS per context with MUI (two bytes a unit) and Babel (one) loaded, and `bundle exec ruby -Ilib test/stress/shared_load.rb multi` | `test/stress/large_load.rb`, whose contexts share nothing, and the same rows under `MINI_RACER_RS_CODE_CACHE=off`, which turns sharing off with the cache |
| Fast calls (`fast_calls`, `runner::fast_call`) | the React workload, which renders through ExecJS's `call`, and `test/stress/large_arguments.rb` | the same build under `MINI_RACER_RS_FAST_CALLS=off`, which sends every eval down the normal path |
| A call's arguments (`crates/mini_racer_engine/src/request.rs`), results written without V8's serializer (`serialize::primitive`), the encoder (`ext/mini_racer_rs/src/encode.rs`) | the call round trip and the serde `ruby_to_js` and `callback` rows | Hashes of 1 to 16 keys, alone and in arrays of 200, sent to a sink and returned: `request::worth_building` leaves a call to V8's deserializer where it builds objects faster than the runner, so a change to either moves where they cross |

p99 and p99.9 rows are often `noisy`. The serde rows below 1x in `BENCHMARKS.md` are arrays of 10,000 numbers. The stress scripts load whatever extension `-Ilib` finds and print one gem's figures with no reference, so use them for before and after on the same build profile only.

## Before and after

1. On `main`, after the setup above, run the exact command you will use afterwards and keep its `tmp/bench` file.
2. Apply the change, `CI=1 bundle exec rake compile`, run the same command.
3. Compare the `candidate` medians of the two files row by row. Each file's ratio column is against a reference run in the same session, which carries over between machines better than absolute times.

## One process under a profiler

Any single benchmark runs by hand as run.rb starts it, for a profiler to wrap: `bundle exec ruby -Ilib bench/measure.rb candidate --quick`, or `EXECJS_RUNTIME=MiniRacer bundle exec ruby -Ilib -Ivendor/execjs/lib bench/workloads.rb candidate babel_transform`. Each prints its JSON to stdout. Where samply is installed (`command -v samply`), `samply record --save-only -o tmp/profile.json.gz -- <command>` records one; it refuses to run while `/proc/sys/kernel/perf_event_paranoid` is above 1.
