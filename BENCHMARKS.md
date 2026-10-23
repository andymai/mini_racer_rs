# Benchmarks

## Method

All tables except the production bundle section use the Ruby 3.4 extension from the published mini_racer_rs 1.5.1 `x86_64-linux-gnu` gem against mini_racer 0.22.2, measured over 5 runs on Ruby 3.4.2 and Linux on an AMD Ryzen 9 7950X3D. The CPU governor was set to `performance`, with both gems pinned to CPUs 0 through 7 using `taskset` (`bench/run.rb --cpus 0-7`). The production bundle section uses commit `2513a48`, measured the same way against mini_racer 0.22.1.

Each run starts mini_racer 0.22.2 with libv8-node 24.12.0.1, then starts mini_racer_rs as a separate process. Tables report medians across runs. Workload tables report the median of each run's median. The harness checks that both gems produce identical output for every workload.

The ratio is how many times faster mini_racer_rs is, or how many times less memory or size it uses. A ratio above 1 favors mini_racer_rs. A row is marked `noisy` when either gem's results spread by more than 10% across runs, so its ratio is less precise. Absolute times depend on the machine, while ratios carry over better.

## Workloads

| case | mini_racer 0.22.2 | mini_racer_rs | ratio | note |
|---|---:|---:|---:|---|
| Babel transform of a 3 KB JSX component, through ExecJS | 9.85 ms | 5.09 ms | 1.93x |  |
| Terser minify of a 78 KB library, through ExecJS | 51.34 ms | 49.85 ms | 1.03x |  |
| React server render of 1,000 items (17 KB of HTML), through ExecJS | 192.46 µs | 174.78 µs | 1.10x |  |
| React in a new context: load the bundle and render 1,000 items, through ExecJS | 11.81 ms | 11.77 ms | 1.00x |  |
| Handlebars render of 50 rows, through `Context#call` | 50.02 µs | 34.96 µs | 1.43x |  |
| Autoprefixer on Bootstrap's 280 KB stylesheet, through ExecJS | 25.98 ms | 24.02 ms | 1.08x |  |
| MUI dashboard server render of 200 rows and 299 icons (469 KB of HTML), through ExecJS | 31.37 ms | 29.22 ms | 1.07x |  |
| MUI in a new context: load the 8.2 MB bundle and render once, through ExecJS | 164.39 ms | 103.98 ms | 1.58x |  |

Each workload runs in its own process. Babel warms up for 50 transforms before 160 timed transforms because V8 keeps optimizing it that long. Babel, Terser, React, Autoprefixer, and MUI run through ExecJS, as Sprockets and react-rails do. Handlebars runs through `Context#call`. ExecJS deletes `console` before loading a source, so the harness provides Babel with a no-op `console`.

The 8.2 MB MUI bundle loads and renders once in a new context in 103.98 ms against 164.39 ms, or 1.58x faster. The 217 KB React bundle is below the threshold and loads in the same time in both gems. Babel's JavaScript takes 5.3 ms per transform inside V8 15.2 and 10.3 ms inside mini_racer's V8 13.6, so its gain comes from V8 rather than call overhead. Terser and React spend nearly all their time in JavaScript and perform about the same. mini_racer_rs compiles evals of 1 MiB or more from a process-wide V8 code cache. The second time a script is seen builds the cache, and later loads compile from it. A context loading the script afterward also reads one source copy shared outside V8's heap.

## A production server rendering bundle

This section uses a 44 MB production server rendering bundle kept outside the repository and loaded through ExecJS in the same way that react-rails loads a bundle. Its table compares commit `2513a48` with mini_racer 0.22.1.

| case | mini_racer 0.22.1 | mini_racer_rs | ratio | note |
|---|---:|---:|---:|---|
| Load a 44 MB production server rendering bundle into a new context, through ExecJS | 982.80 ms | 215.01 ms | 4.57x |  |
| RSS per context with that bundle loaded, 5 live | 300.93 MB | 135.89 MB | 2.21x |  |

The bundle loads into a new context in 215.01 ms against 982.80 ms, or 4.57x faster, because mini_racer_rs compiles it from the code cache from its third load on. With 5 contexts holding it, each uses 135.89 MB of resident memory against 300.93 MB, or 2.21x less. After an eval of 1 MiB or more, a snapshot build, or a low-memory collection, mini_racer_rs returns freed malloc memory to the system with `malloc_trim` on glibc. To include another bundle, set `MINI_RACER_RS_BENCH_SSR_BUNDLE` to its file, `MINI_RACER_RS_BENCH_SSR_PRELUDE` to a file containing the globals it needs first, and `MINI_RACER_RS_BENCH_SSR_CALL` to the JavaScript expression that renders. The output omits the bundle's path, name, and error messages.

## Per-call costs, startup and payloads

| case | mini_racer 0.22.2 | mini_racer_rs | ratio | note |
|---|---:|---:|---:|---|
| Eval round trip p50 | 6.58 µs | 1.13 µs | 5.81x |  |
| Eval round trip p99 | 10.29 µs | 2.78 µs | 3.69x |  |
| Eval round trip p99.9 | 62.20 µs | 14.16 µs | 4.39x |  |
| Call round trip p50 | 6.50 µs | 0.75 µs | 8.65x |  |
| Call round trip p99 | 10.43 µs | 0.99 µs | 10.51x | noisy |
| Call round trip p99.9 | 79.35 µs | 3.38 µs | 23.50x | noisy |
| Call with timeout p50 | 20.42 µs | 0.78 µs | 26.11x |  |
| Call with timeout p99 | 39.91 µs | 0.98 µs | 40.65x | noisy |
| Call with timeout p99.9 | 169.45 µs | 3.27 µs | 51.88x | noisy |
| Call in a timeout scope p50 | 20.22 µs | 0.87 µs | 23.21x |  |
| Call in a timeout scope p99 | 35.53 µs | 1.09 µs | 32.53x | noisy |
| Call in a timeout scope p99.9 | 150.25 µs | 3.54 µs | 42.48x | noisy |
| Callback round trip p50 | 6.13 µs | 1.11 µs | 5.54x |  |
| Startup p50 | 1.59 ms | 1.12 ms | 1.42x |  |
| Startup and load Babel p50 | 44.83 ms | 13.34 ms | 3.36x |  |
| Startup from a Babel snapshot p50 | 11.00 ms | 5.02 ms | 2.19x |  |
| Babel snapshot size | 5.68 MB | 5.76 MB | 0.99x |  |
| JS to Ruby | 52.22 MB/s | 166.99 MB/s | 3.20x |  |
| Ruby to JS | 181.26 MB/s | 289.32 MB/s | 1.60x |  |
| Idle CPU, 50 contexts | 0.00 % of a core | 0.00 % of a core | n/a | under 1.0 |

mini_racer releases the GVL and passes each request and reply through a mutex and condition variable. mini_racer_rs briefly spins its runner and Ruby thread before parking, so quick replies avoid a sleep and wake. Idle contexts use no CPU. For a context with `timeout:`, mini_racer starts a watchdog thread for every eval or call, while mini_racer_rs uses one watchdog thread per process. For the "Call in a timeout scope" rows, mini_racer measures a call on a context with `timeout:`, its only bound on one call, while mini_racer_rs measures a call on a plain context inside `MiniRacerRs.timeout`.

Startup plus loading Babel takes 13.34 ms against 44.83 ms, or 3.36x faster. Babel's 2.3 MB bundle compiles from the code cache.

## Memory

Each memory scenario runs in a fresh process. Resident memory is read after garbage collections, a pause, and one more collection, so released contexts have finished tearing down. Memory retained by malloc after a free still counts, as it does for an application. Per-context rows leave out the first context, which pays for starting V8. The ratio is how many times less memory mini_racer_rs uses, and a value above 1 favors mini_racer_rs.

| case | mini_racer 0.22.2 | mini_racer_rs | ratio | note |
|---|---:|---:|---:|---|
| RSS of a process with one idle context | 39.70 MB | 43.63 MB | 0.91x |  |
| RSS per idle context, 100 live | 1.39 MB | 1.58 MB | 0.88x |  |
| RSS per context with Babel loaded, 20 live | 28.51 MB | 15.00 MB | 1.90x |  |
| RSS per context from one Babel snapshot, 20 live | 17.37 MB | 9.21 MB | 1.89x |  |
| RSS per context with the MUI bundle loaded, 20 live | 110.38 MB | 43.48 MB | 2.54x |  |
| RSS growth per context over 20,000 created and disposed, after 2,000 | 1,619.15 bytes | 146.02 bytes | 11.09x | noisy |
| Peak RSS over 20,000 contexts left to the garbage collector | 2,918.03 MB | 107.74 MB | 27.08x |  |

Loaded contexts use less memory: Babel uses 15.00 MB against 28.51 MB, a context from one Babel snapshot uses 9.21 MB against 17.37 MB, and MUI uses 43.48 MB against 110.38 MB. An idle context costs 1.58 MB against 1.39 MB, or 0.88x. Contexts loading the same script of 1 MiB or more read one shared copy of its source, and contexts from one snapshot share its blob. Creating and disposing 20,000 contexts grows resident memory by 146.02 bytes per context against 1,619.15 bytes, although this row is noisy. When 20,000 contexts are left to Ruby's garbage collector, resident memory peaks at 107.74 MB against 2,918.03 MB. mini_racer_rs tells Ruby's garbage collector that each live context holds 2 MiB outside the Ruby heap with `rb_gc_adjust_memory_usage`, so collections run and finalize abandoned contexts sooner. mini_racer reports only the size of its context struct.

## mini_racer's own benchmark/serde suite

Times are microseconds per iteration. The suite's context has a 120 second timeout, so every case includes the cost of arming it.

| case | mini_racer 0.22.2 | mini_racer_rs | ratio | note |
|---|---:|---:|---:|---|
| `js_to_ruby/return_10k_ints` | 226.81 | 137.60 | 1.65x |  |
| `js_to_ruby/return_10k_floats` | 208.33 | 73.68 | 2.83x |  |
| `js_to_ruby/return_10k_ascii_strings` | 1742.46 | 643.25 | 2.71x | noisy |
| `js_to_ruby/return_10k_latin1_strings` | 5752.46 | 595.65 | 9.66x |  |
| `js_to_ruby/return_10k_utf8_bmp_strings` | 5947.13 | 618.68 | 9.61x |  |
| `js_to_ruby/return_10k_utf8_emoji_strings` | 6012.04 | 647.64 | 9.28x |  |
| `js_to_ruby/return_object_1k_ints` | 399.31 | 196.87 | 2.03x |  |
| `js_to_ruby/return_array_1k_objects` | 801.20 | 301.69 | 2.66x |  |
| `ruby_to_js/call_no_args` | 21.19 | 0.74 | 28.62x |  |
| `ruby_to_js/call_int_arg` | 21.65 | 0.79 | 27.44x |  |
| `ruby_to_js/call_short_string` | 21.99 | 1.02 | 21.50x |  |
| `ruby_to_js/call_utf8_string` | 24.08 | 1.08 | 22.34x |  |
| `ruby_to_js/call_emoji_string` | 23.91 | 1.10 | 21.77x |  |
| `ruby_to_js/roundtrip_10k_ints` | 570.95 | 297.84 | 1.92x | noisy |
| `ruby_to_js/roundtrip_10k_floats` | 628.41 | 309.71 | 2.03x |  |
| `ruby_to_js/roundtrip_10k_strings` | 2342.82 | 847.34 | 2.76x | noisy |
| `ruby_to_js/roundtrip_10k_utf8_bmp_strings` | 6390.55 | 1021.69 | 6.25x |  |
| `ruby_to_js/roundtrip_10k_utf8_emoji_strings` | 6473.74 | 1052.80 | 6.15x |  |
| `ruby_to_js/roundtrip_hash_1k` | 354.08 | 164.35 | 2.15x |  |
| `ruby_to_js/roundtrip_array_1k_hashes` | 1189.79 | 534.40 | 2.23x |  |
| `callback/1000_echo_int` | 6090.24 | 1050.47 | 5.80x |  |
| `callback/1000_echo_short_string` | 6196.68 | 1146.76 | 5.40x | noisy |
| `callback/1000_echo_utf8_string` | 6419.29 | 1204.64 | 5.33x |  |
| `callback/1000_echo_emoji_string` | 6418.50 | 1198.36 | 5.36x |  |
| `callback/1000_add_ints` | 6226.01 | 1139.41 | 5.46x |  |

Every row favors mini_racer_rs, from 1.65x to 28.62x. Arrays of numbers decode from JavaScript to Ruby in runs of up to 64 elements, object keys are interned and each Hash is filled with one bulk insert, calls with few properties are built by the runner, and primitive results are written without V8's serializer.

## Reproducing

Install mini_racer 0.22.2 under `tools/reference`:

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

Build the shipped release extension without debug info or local symbols, matching the published gem's extension size:

```sh
CI=1 CARGO_PROFILE_RELEASE_DEBUG=false CARGO_PROFILE_RELEASE_STRIP=symbols bundle exec rake compile
```

Run every table 5 times, pinned to CPUs 0 through 7:

```sh
bundle exec ruby bench/run.rb --cpus 0-7
```

`--runs N` changes the run count. `--quick` makes one short smoke-test run of each case. `--no-micro`, `--no-serde`, `--no-workloads`, and `--no-memory` skip a group.

Workloads need current consumer bundles. The runner names the build command when they are missing or stale. Results are written to `tmp/bench/<timestamp>.json`.
