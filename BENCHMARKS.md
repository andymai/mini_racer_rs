# Benchmarks

## Method

All tables come from one release build of commit `2513a48`, measured over 5 runs on Ruby 3.4.2 and Linux on an AMD Ryzen 9 7950X3D. The CPU governor is set to `performance`, and both gems are pinned to CPUs 0 through 7 with `taskset` (`bench/run.rb --cpus 0-7`).

Each run starts mini_racer 0.22.1 with libv8-node 24.12.0.1, then starts mini_racer_rs as a separate process. Tables report medians across runs. Workload tables report the median of each run's median. The harness checks that both gems produce identical output for every workload.

The ratio is how many times faster mini_racer_rs is, or how many times less memory or size it uses. A ratio above 1 favors mini_racer_rs. A row is marked `noisy` when either gem's results spread by more than 10% across runs, so its ratio is less precise. Absolute times depend on the machine, while ratios carry over better.

## Workloads

| case | mini_racer 0.22.1 | mini_racer_rs | ratio | note |
|---|---:|---:|---:|---|
| Babel transform of a 3 KB JSX component, through ExecJS | 10.00 ms | 5.32 ms | 1.88x |  |
| Terser minify of a 78 KB library, through ExecJS | 51.65 ms | 51.23 ms | 1.01x | noisy |
| React server render of 1,000 items (17 KB of HTML), through ExecJS | 192.31 µs | 173.63 µs | 1.11x | noisy |
| React in a new context: load the bundle and render 1,000 items, through ExecJS | 12.00 ms | 12.05 ms | 1.00x | noisy |
| Handlebars render of 50 rows, through `Context#call` | 47.95 µs | 40.14 µs | 1.19x |  |
| Autoprefixer on Bootstrap's 280 KB stylesheet, through ExecJS | 27.23 ms | 23.71 ms | 1.15x |  |
| MUI dashboard server render of 200 rows and 299 icons (469 KB of HTML), through ExecJS | 32.19 ms | 30.55 ms | 1.05x |  |
| MUI in a new context: load the 8.2 MB bundle and render once, through ExecJS | 166.72 ms | 120.64 ms | 1.38x |  |

Each workload runs in its own process. Babel warms up for 50 transforms before 160 timed transforms because V8 keeps optimizing it that long. Babel, Terser, React, Autoprefixer, and MUI run through ExecJS, as Sprockets and react-rails do. Handlebars runs through `Context#call`. ExecJS deletes `console` before loading a source, so the harness provides Babel with a no-op `console`.

Babel's JavaScript takes 5.3 ms per transform inside V8 15.2 and 10.3 ms inside mini_racer's V8 13.6, so its gain comes from V8 rather than call overhead. Terser and React spend nearly all their time in JavaScript and perform about the same. mini_racer_rs compiles evals of 1 MB or more from a process-wide V8 code cache. The second time a script is seen builds the cache, and later loads compile from it. The 8.2 MB MUI bundle therefore loads and renders once in a new context in 120.64 ms against 166.72 ms, or 1.38x faster. The 217 KB React bundle is below the threshold and loads in the same time in both gems.

## A production server rendering bundle

This benchmark uses a 44 MB production server rendering bundle kept outside the repository. It is loaded through ExecJS in the same way that react-rails loads a bundle.

| case | mini_racer 0.22.1 | mini_racer_rs | ratio | note |
|---|---:|---:|---:|---|
| Load a 44 MB production server rendering bundle into a new context, through ExecJS | 982.80 ms | 215.01 ms | 4.57x |  |
| RSS per context with that bundle loaded, 5 live | 300.93 MB | 135.89 MB | 2.21x |  |

The bundle loads into a new context in 215.01 ms against 982.80 ms, or 4.57x faster, because mini_racer_rs compiles it from the code cache from its third load on. With 5 contexts holding it, each uses 135.89 MB of resident memory against 300.93 MB, or 2.21x less. After an eval of 1 MiB or more, a snapshot build, or a low-memory collection, mini_racer_rs returns freed malloc memory to the system with `malloc_trim` on glibc. To include another bundle, set `MINI_RACER_RS_BENCH_SSR_BUNDLE` to its file, `MINI_RACER_RS_BENCH_SSR_PRELUDE` to a file containing the globals it needs first, and `MINI_RACER_RS_BENCH_SSR_CALL` to the JavaScript expression that renders. The output omits the bundle's path, name, and error messages.

## Per-call costs, startup and payloads

| case | mini_racer 0.22.1 | mini_racer_rs | ratio | note |
|---|---:|---:|---:|---|
| Eval round trip p50 | 6.64 µs | 1.30 µs | 5.10x | noisy |
| Eval round trip p99 | 10.61 µs | 2.98 µs | 3.57x | noisy |
| Eval round trip p99.9 | 62.08 µs | 16.94 µs | 3.66x | noisy |
| Call round trip p50 | 6.51 µs | 1.55 µs | 4.19x |  |
| Call round trip p99 | 11.02 µs | 2.40 µs | 4.58x | noisy |
| Call round trip p99.9 | 81.01 µs | 16.04 µs | 5.05x | noisy |
| Call with timeout p50 | 21.12 µs | 1.61 µs | 13.09x |  |
| Call with timeout p99 | 43.25 µs | 2.48 µs | 17.48x | noisy |
| Call with timeout p99.9 | 174.71 µs | 15.94 µs | 10.96x | noisy |
| Callback round trip p50 | 6.14 µs | 1.27 µs | 4.85x |  |
| Startup p50 | 1.51 ms | 1.06 ms | 1.42x |  |
| Startup and load Babel p50 | 44.84 ms | 14.83 ms | 3.02x | noisy |
| Startup from a Babel snapshot p50 | 11.44 ms | 5.09 ms | 2.25x |  |
| Babel snapshot size | 5.68 MB | 5.76 MB | 0.99x |  |
| JS to Ruby | 52.17 MB/s | 114.09 MB/s | 2.19x |  |
| Ruby to JS | 181.11 MB/s | 199.51 MB/s | 1.10x |  |
| Idle CPU, 50 contexts | 0.00 % of a core | 0.00 % of a core | n/a | under 1.0 |

mini_racer releases the GVL and passes each request and reply through a mutex and condition variable. mini_racer_rs briefly spins its runner and Ruby thread before parking, so quick replies avoid a sleep and wake. Idle contexts use no CPU. For a context with `timeout:`, mini_racer starts a watchdog thread for every eval or call, while mini_racer_rs uses one watchdog thread per process.

Startup plus loading Babel takes 14.83 ms against 44.84 ms, or 3.02x faster. This row is noisy. Babel's 2.3 MB bundle compiles from the code cache.

## Memory

Each memory scenario runs in a fresh process. Resident memory is read after garbage collections, a pause, and one more collection, so released contexts have finished tearing down. Memory retained by malloc after a free still counts, as it does for an application. Per-context rows leave out the first context, which pays for starting V8. The ratio is how many times less memory mini_racer_rs uses, and a value above 1 favors mini_racer_rs.

| case | mini_racer 0.22.1 | mini_racer_rs | ratio | note |
|---|---:|---:|---:|---|
| RSS of a process with one idle context | 39.68 MB | 38.01 MB | 1.04x |  |
| RSS per idle context, 100 live | 1.39 MB | 1.58 MB | 0.88x |  |
| RSS per context with Babel loaded, 20 live | 28.52 MB | 17.21 MB | 1.66x |  |
| RSS per context with the MUI bundle loaded, 20 live | 110.33 MB | 55.51 MB | 1.99x |  |
| RSS growth per context over 20,000 created and disposed, after 2,000 | 1,606.86 bytes | 196.61 bytes | 8.17x | noisy |
| Peak RSS over 20,000 contexts left to the garbage collector | 2,915.84 MB | 101.88 MB | 28.62x |  |

An idle context costs 1.58 MB against 1.39 MB, or 0.88x. Loaded contexts use less memory: Babel uses 17.21 MB against 28.52 MB, and MUI uses 55.51 MB against 110.33 MB. Creating and disposing 20,000 contexts grows resident memory by 196.61 bytes per context against 1,606.86 bytes, although this row is noisy. When 20,000 contexts are left to Ruby's garbage collector, resident memory peaks at 101.88 MB against 2,915.84 MB. mini_racer_rs tells Ruby's garbage collector that each live context holds 2 MiB outside the Ruby heap with `rb_gc_adjust_memory_usage`, so collections run and finalize abandoned contexts sooner. mini_racer reports only the size of its context struct.

## mini_racer's own benchmark/serde suite

Times are microseconds per iteration. The suite's context has a 120 second timeout, so every case includes the cost of arming it.

| case | mini_racer 0.22.1 | mini_racer_rs | ratio | note |
|---|---:|---:|---:|---|
| `js_to_ruby/return_10k_ints` | 228.60 | 141.72 | 1.61x | noisy |
| `js_to_ruby/return_10k_floats` | 208.62 | 77.54 | 2.69x | noisy |
| `js_to_ruby/return_10k_ascii_strings` | 1708.98 | 652.43 | 2.62x |  |
| `js_to_ruby/return_10k_latin1_strings` | 5796.54 | 687.24 | 8.43x | noisy |
| `js_to_ruby/return_10k_utf8_bmp_strings` | 5847.54 | 731.20 | 8.00x |  |
| `js_to_ruby/return_10k_utf8_emoji_strings` | 5987.57 | 766.52 | 7.81x |  |
| `js_to_ruby/return_object_1k_ints` | 381.68 | 226.46 | 1.69x | noisy |
| `js_to_ruby/return_array_1k_objects` | 792.20 | 409.43 | 1.93x |  |
| `ruby_to_js/call_no_args` | 22.31 | 1.56 | 14.33x |  |
| `ruby_to_js/call_int_arg` | 22.72 | 1.63 | 13.94x |  |
| `ruby_to_js/call_short_string` | 23.28 | 1.81 | 12.87x |  |
| `ruby_to_js/call_utf8_string` | 25.19 | 1.82 | 13.80x |  |
| `ruby_to_js/call_emoji_string` | 25.22 | 1.84 | 13.72x |  |
| `ruby_to_js/roundtrip_10k_ints` | 562.31 | 558.40 | 1.01x |  |
| `ruby_to_js/roundtrip_10k_floats` | 612.24 | 590.90 | 1.04x |  |
| `ruby_to_js/roundtrip_10k_strings` | 2366.30 | 1239.04 | 1.91x |  |
| `ruby_to_js/roundtrip_10k_utf8_bmp_strings` | 6433.81 | 1385.94 | 4.64x |  |
| `ruby_to_js/roundtrip_10k_utf8_emoji_strings` | 6536.89 | 1459.54 | 4.48x |  |
| `ruby_to_js/roundtrip_hash_1k` | 347.90 | 196.05 | 1.77x | noisy |
| `ruby_to_js/roundtrip_array_1k_hashes` | 1162.93 | 730.33 | 1.59x | noisy |
| `callback/1000_echo_int` | 6064.60 | 1212.82 | 5.00x | noisy |
| `callback/1000_echo_short_string` | 6203.71 | 1314.65 | 4.72x | noisy |
| `callback/1000_echo_utf8_string` | 6405.00 | 1339.60 | 4.78x |  |
| `callback/1000_echo_emoji_string` | 6494.21 | 1364.03 | 4.76x | noisy |
| `callback/1000_add_ints` | 6239.86 | 1291.11 | 4.83x |  |

Every row favors mini_racer_rs, from 1.01x to 14.33x. Arrays of numbers decode from JavaScript to Ruby in runs of up to 64 elements.

## Reproducing

Install mini_racer 0.22.1 under `tools/reference`:

```sh
cd tools/reference
BUNDLE_PATH=vendor/bundle bundle install
```

Build the consumer bundles:

```sh
cd test/consumers/js
npm ci
npm run build
```

Build the release extension:

```sh
CI=1 bundle exec rake compile
```

Run every table 5 times, pinned to CPUs 0 through 7:

```sh
bundle exec ruby bench/run.rb --cpus 0-7
```

`--runs N` changes the run count. `--quick` makes one short smoke-test run of each case. `--no-micro`, `--no-serde`, `--no-workloads`, and `--no-memory` skip a group.

Workloads need current consumer bundles. The runner names the build command when they are missing or stale. Results are written to `tmp/bench/<timestamp>.json`.
