# Benchmarks

## Method

All tables except the production bundle section come from one release build of commit `220bfe7`, measured over 5 runs on Ruby 3.4.2 and Linux on an AMD Ryzen 9 7950X3D, with the CPU governor set to `performance` and both gems pinned to CPUs 0 through 7 with `taskset` (`bench/run.rb --cpus 0-7`), while the production bundle section comes from commit `2513a48`, measured the same way.

Each run starts mini_racer 0.22.1 with libv8-node 24.12.0.1, then starts mini_racer_rs as a separate process. Tables report medians across runs. Workload tables report the median of each run's median. The harness checks that both gems produce identical output for every workload.

The ratio is how many times faster mini_racer_rs is, or how many times less memory or size it uses. A ratio above 1 favors mini_racer_rs. A row is marked `noisy` when either gem's results spread by more than 10% across runs, so its ratio is less precise. Absolute times depend on the machine, while ratios carry over better.

## Workloads

| case | mini_racer 0.22.1 | mini_racer_rs | ratio | note |
|---|---:|---:|---:|---|
| Babel transform of a 3 KB JSX component, through ExecJS | 10.05 ms | 5.15 ms | 1.95x | noisy |
| Terser minify of a 78 KB library, through ExecJS | 51.07 ms | 51.97 ms | 0.98x | noisy |
| React server render of 1,000 items (17 KB of HTML), through ExecJS | 191.28 µs | 173.63 µs | 1.10x | noisy |
| React in a new context: load the bundle and render 1,000 items, through ExecJS | 11.90 ms | 11.84 ms | 1.01x | noisy |
| Handlebars render of 50 rows, through `Context#call` | 49.11 µs | 34.86 µs | 1.41x | noisy |
| Autoprefixer on Bootstrap's 280 KB stylesheet, through ExecJS | 26.31 ms | 23.73 ms | 1.11x | noisy |
| MUI dashboard server render of 200 rows and 299 icons (469 KB of HTML), through ExecJS | 32.56 ms | 29.75 ms | 1.09x |  |
| MUI in a new context: load the 8.2 MB bundle and render once, through ExecJS | 166.77 ms | 105.14 ms | 1.59x |  |

Each workload runs in its own process. Babel warms up for 50 transforms before 160 timed transforms because V8 keeps optimizing it that long. Babel, Terser, React, Autoprefixer, and MUI run through ExecJS, as Sprockets and react-rails do. Handlebars runs through `Context#call`. ExecJS deletes `console` before loading a source, so the harness provides Babel with a no-op `console`.

The 8.2 MB MUI bundle loads and renders once in a new context in 105.14 ms against 166.77 ms, or 1.59x faster. The 217 KB React bundle is below the threshold and loads in the same time in both gems. Babel's JavaScript takes 5.3 ms per transform inside V8 15.2 and 10.3 ms inside mini_racer's V8 13.6, so its gain comes from V8 rather than call overhead. Terser and React spend nearly all their time in JavaScript and perform about the same. mini_racer_rs compiles evals of 1 MB or more from a process-wide V8 code cache. The second time a script is seen builds the cache, and later loads compile from it. A context loading the script afterward also reads one source copy shared outside V8's heap.

## A production server rendering bundle

This section uses a 44 MB production server rendering bundle kept outside the repository and loaded through ExecJS in the same way that react-rails loads a bundle. Its table comes from commit `2513a48`.

| case | mini_racer 0.22.1 | mini_racer_rs | ratio | note |
|---|---:|---:|---:|---|
| Load a 44 MB production server rendering bundle into a new context, through ExecJS | 982.80 ms | 215.01 ms | 4.57x |  |
| RSS per context with that bundle loaded, 5 live | 300.93 MB | 135.89 MB | 2.21x |  |

The bundle loads into a new context in 215.01 ms against 982.80 ms, or 4.57x faster, because mini_racer_rs compiles it from the code cache from its third load on. With 5 contexts holding it, each uses 135.89 MB of resident memory against 300.93 MB, or 2.21x less. After an eval of 1 MiB or more, a snapshot build, or a low-memory collection, mini_racer_rs returns freed malloc memory to the system with `malloc_trim` on glibc. To include another bundle, set `MINI_RACER_RS_BENCH_SSR_BUNDLE` to its file, `MINI_RACER_RS_BENCH_SSR_PRELUDE` to a file containing the globals it needs first, and `MINI_RACER_RS_BENCH_SSR_CALL` to the JavaScript expression that renders. The output omits the bundle's path, name, and error messages.

## Per-call costs, startup and payloads

| case | mini_racer 0.22.1 | mini_racer_rs | ratio | note |
|---|---:|---:|---:|---|
| Eval round trip p50 | 6.91 µs | 1.09 µs | 6.33x |  |
| Eval round trip p99 | 10.10 µs | 2.72 µs | 3.71x |  |
| Eval round trip p99.9 | 61.38 µs | 14.04 µs | 4.37x | noisy |
| Call round trip p50 | 6.51 µs | 0.72 µs | 9.03x |  |
| Call round trip p99 | 12.81 µs | 1.08 µs | 11.84x | noisy |
| Call round trip p99.9 | 80.69 µs | 3.33 µs | 24.26x | noisy |
| Call with timeout p50 | 20.48 µs | 0.76 µs | 26.87x | noisy |
| Call with timeout p99 | 35.52 µs | 1.16 µs | 30.57x | noisy |
| Call with timeout p99.9 | 164.01 µs | 3.29 µs | 49.91x | noisy |
| Callback round trip p50 | 6.12 µs | 1.12 µs | 5.44x | noisy |
| Startup p50 | 1.56 ms | 1.09 ms | 1.43x |  |
| Startup and load Babel p50 | 44.41 ms | 13.26 ms | 3.35x |  |
| Startup from a Babel snapshot p50 | 11.04 ms | 5.04 ms | 2.19x |  |
| Babel snapshot size | 5.68 MB | 5.76 MB | 0.99x |  |
| JS to Ruby | 52.68 MB/s | 171.33 MB/s | 3.25x |  |
| Ruby to JS | 181.74 MB/s | 248.35 MB/s | 1.37x |  |
| Idle CPU, 50 contexts | 0.00 % of a core | 0.00 % of a core | n/a | under 1.0 |

mini_racer releases the GVL and passes each request and reply through a mutex and condition variable. mini_racer_rs briefly spins its runner and Ruby thread before parking, so quick replies avoid a sleep and wake. Idle contexts use no CPU. For a context with `timeout:`, mini_racer starts a watchdog thread for every eval or call, while mini_racer_rs uses one watchdog thread per process.

Startup plus loading Babel takes 13.26 ms against 44.41 ms, or 3.35x faster. Babel's 2.3 MB bundle compiles from the code cache.

## Memory

Each memory scenario runs in a fresh process. Resident memory is read after garbage collections, a pause, and one more collection, so released contexts have finished tearing down. Memory retained by malloc after a free still counts, as it does for an application. Per-context rows leave out the first context, which pays for starting V8. The ratio is how many times less memory mini_racer_rs uses, and a value above 1 favors mini_racer_rs.

| case | mini_racer 0.22.1 | mini_racer_rs | ratio | note |
|---|---:|---:|---:|---|
| RSS of a process with one idle context | 39.69 MB | 37.94 MB | 1.05x |  |
| RSS per idle context, 100 live | 1.39 MB | 1.58 MB | 0.88x |  |
| RSS per context with Babel loaded, 20 live | 28.50 MB | 15.15 MB | 1.88x |  |
| RSS per context from one Babel snapshot, 20 live | 17.37 MB | 9.20 MB | 1.89x |  |
| RSS per context with the MUI bundle loaded, 20 live | 110.34 MB | 44.25 MB | 2.49x |  |
| RSS growth per context over 20,000 created and disposed, after 2,000 | 1,659.29 bytes | 101.38 bytes | 16.37x | noisy |
| Peak RSS over 20,000 contexts left to the garbage collector | 2,919.62 MB | 102.05 MB | 28.61x |  |

Loaded contexts use less memory: Babel uses 15.15 MB against 28.50 MB, a context from one Babel snapshot uses 9.20 MB against 17.37 MB, and MUI uses 44.25 MB against 110.34 MB. An idle context costs 1.58 MB against 1.39 MB, or 0.88x. Contexts loading the same script of 1 MiB or more read one shared copy of its source, and contexts from one snapshot share its blob. Creating and disposing 20,000 contexts grows resident memory by 101.38 bytes per context against 1,659.29 bytes, although this row is noisy. When 20,000 contexts are left to Ruby's garbage collector, resident memory peaks at 102.05 MB against 2,919.62 MB. mini_racer_rs tells Ruby's garbage collector that each live context holds 2 MiB outside the Ruby heap with `rb_gc_adjust_memory_usage`, so collections run and finalize abandoned contexts sooner. mini_racer reports only the size of its context struct.

## mini_racer's own benchmark/serde suite

Times are microseconds per iteration. The suite's context has a 120 second timeout, so every case includes the cost of arming it.

| case | mini_racer 0.22.1 | mini_racer_rs | ratio | note |
|---|---:|---:|---:|---|
| `js_to_ruby/return_10k_ints` | 227.20 | 134.82 | 1.69x |  |
| `js_to_ruby/return_10k_floats` | 203.47 | 71.06 | 2.86x |  |
| `js_to_ruby/return_10k_ascii_strings` | 1679.34 | 615.89 | 2.73x | noisy |
| `js_to_ruby/return_10k_latin1_strings` | 5789.52 | 580.29 | 9.98x |  |
| `js_to_ruby/return_10k_utf8_bmp_strings` | 5882.58 | 627.59 | 9.37x |  |
| `js_to_ruby/return_10k_utf8_emoji_strings` | 5941.93 | 662.80 | 8.96x | noisy |
| `js_to_ruby/return_object_1k_ints` | 374.80 | 202.95 | 1.85x | noisy |
| `js_to_ruby/return_array_1k_objects` | 784.38 | 286.29 | 2.74x | noisy |
| `ruby_to_js/call_no_args` | 21.66 | 0.73 | 29.78x |  |
| `ruby_to_js/call_int_arg` | 21.80 | 0.78 | 27.97x |  |
| `ruby_to_js/call_short_string` | 22.56 | 1.03 | 21.95x | noisy |
| `ruby_to_js/call_utf8_string` | 24.07 | 1.06 | 22.69x |  |
| `ruby_to_js/call_emoji_string` | 24.18 | 1.07 | 22.67x |  |
| `ruby_to_js/roundtrip_10k_ints` | 564.05 | 285.51 | 1.98x | noisy |
| `ruby_to_js/roundtrip_10k_floats` | 617.37 | 303.25 | 2.04x | noisy |
| `ruby_to_js/roundtrip_10k_strings` | 2356.94 | 849.73 | 2.77x | noisy |
| `ruby_to_js/roundtrip_10k_utf8_bmp_strings` | 6469.14 | 1021.49 | 6.33x |  |
| `ruby_to_js/roundtrip_10k_utf8_emoji_strings` | 6491.21 | 1042.02 | 6.23x |  |
| `ruby_to_js/roundtrip_hash_1k` | 346.39 | 166.83 | 2.08x |  |
| `ruby_to_js/roundtrip_array_1k_hashes` | 1172.25 | 526.73 | 2.23x |  |
| `callback/1000_echo_int` | 6093.58 | 1050.95 | 5.80x |  |
| `callback/1000_echo_short_string` | 6207.97 | 1157.08 | 5.37x |  |
| `callback/1000_echo_utf8_string` | 6486.89 | 1176.41 | 5.51x |  |
| `callback/1000_echo_emoji_string` | 6380.80 | 1182.71 | 5.40x | noisy |
| `callback/1000_add_ints` | 6239.26 | 1153.16 | 5.41x | noisy |

Every row favors mini_racer_rs, from 1.69x to 29.78x. Arrays of numbers decode from JavaScript to Ruby in runs of up to 64 elements, object keys are interned and each Hash is filled with one bulk insert, calls with few properties are built by the runner, and primitive results are written without V8's serializer.

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
