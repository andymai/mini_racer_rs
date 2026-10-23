# Benchmarks

## Method

All tables except the production bundle section use the Ruby 3.4 extension from the published mini_racer_rs 1.4.0 `x86_64-linux-gnu` gem, measured over 5 runs on Ruby 3.4.2 and Linux on an AMD Ryzen 9 7950X3D. The CPU governor was set to `performance`, with both gems pinned to CPUs 0 through 7 using `taskset` (`bench/run.rb --cpus 0-7`). The production bundle section uses commit `2513a48`, measured the same way.

Each run starts mini_racer 0.22.1 with libv8-node 24.12.0.1, then starts mini_racer_rs as a separate process. Tables report medians across runs. Workload tables report the median of each run's median. The harness checks that both gems produce identical output for every workload.

The ratio is how many times faster mini_racer_rs is, or how many times less memory or size it uses. A ratio above 1 favors mini_racer_rs. A row is marked `noisy` when either gem's results spread by more than 10% across runs, so its ratio is less precise. Absolute times depend on the machine, while ratios carry over better.

## Workloads

| case | mini_racer 0.22.1 | mini_racer_rs | ratio | note |
|---|---:|---:|---:|---|
| Babel transform of a 3 KB JSX component, through ExecJS | 10.06 ms | 5.11 ms | 1.97x |  |
| Terser minify of a 78 KB library, through ExecJS | 51.39 ms | 50.86 ms | 1.01x |  |
| React server render of 1,000 items (17 KB of HTML), through ExecJS | 192.43 µs | 181.61 µs | 1.06x |  |
| React in a new context: load the bundle and render 1,000 items, through ExecJS | 11.80 ms | 11.75 ms | 1.00x |  |
| Handlebars render of 50 rows, through `Context#call` | 49.05 µs | 34.66 µs | 1.42x |  |
| Autoprefixer on Bootstrap's 280 KB stylesheet, through ExecJS | 26.25 ms | 23.80 ms | 1.10x |  |
| MUI dashboard server render of 200 rows and 299 icons (469 KB of HTML), through ExecJS | 30.33 ms | 29.19 ms | 1.04x | noisy |
| MUI in a new context: load the 8.2 MB bundle and render once, through ExecJS | 165.90 ms | 103.57 ms | 1.60x |  |

Each workload runs in its own process. Babel warms up for 50 transforms before 160 timed transforms because V8 keeps optimizing it that long. Babel, Terser, React, Autoprefixer, and MUI run through ExecJS, as Sprockets and react-rails do. Handlebars runs through `Context#call`. ExecJS deletes `console` before loading a source, so the harness provides Babel with a no-op `console`.

The 8.2 MB MUI bundle loads and renders once in a new context in 103.57 ms against 165.90 ms, or 1.60x faster. The 217 KB React bundle is below the threshold and loads in the same time in both gems. Babel's JavaScript takes 5.3 ms per transform inside V8 15.2 and 10.3 ms inside mini_racer's V8 13.6, so its gain comes from V8 rather than call overhead. Terser and React spend nearly all their time in JavaScript and perform about the same. mini_racer_rs compiles evals of 1 MiB or more from a process-wide V8 code cache. The second time a script is seen builds the cache, and later loads compile from it. A context loading the script afterward also reads one source copy shared outside V8's heap.

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
| Eval round trip p50 | 6.76 µs | 1.14 µs | 5.92x |  |
| Eval round trip p99 | 10.58 µs | 2.78 µs | 3.81x | noisy |
| Eval round trip p99.9 | 63.94 µs | 13.44 µs | 4.76x | noisy |
| Call round trip p50 | 6.45 µs | 0.75 µs | 8.59x |  |
| Call round trip p99 | 10.51 µs | 1.12 µs | 9.37x | noisy |
| Call round trip p99.9 | 79.55 µs | 3.37 µs | 23.63x | noisy |
| Call with timeout p50 | 20.50 µs | 0.78 µs | 26.21x |  |
| Call with timeout p99 | 39.42 µs | 1.07 µs | 36.78x | noisy |
| Call with timeout p99.9 | 165.51 µs | 3.34 µs | 49.61x | noisy |
| Call in a timeout scope p50 | 20.61 µs | 0.85 µs | 24.19x |  |
| Call in a timeout scope p99 | 38.87 µs | 1.09 µs | 35.60x | noisy |
| Call in a timeout scope p99.9 | 187.64 µs | 3.50 µs | 53.67x | noisy |
| Callback round trip p50 | 6.06 µs | 1.11 µs | 5.48x |  |
| Startup p50 | 1.56 ms | 1.10 ms | 1.41x | noisy |
| Startup and load Babel p50 | 44.29 ms | 13.20 ms | 3.36x |  |
| Startup from a Babel snapshot p50 | 10.76 ms | 5.03 ms | 2.14x |  |
| Babel snapshot size | 5.68 MB | 5.76 MB | 0.99x |  |
| JS to Ruby | 53.17 MB/s | 170.15 MB/s | 3.20x |  |
| Ruby to JS | 180.66 MB/s | 298.24 MB/s | 1.65x |  |
| Idle CPU, 50 contexts | 0.00 % of a core | 0.00 % of a core | n/a | under 1.0 |

mini_racer releases the GVL and passes each request and reply through a mutex and condition variable. mini_racer_rs briefly spins its runner and Ruby thread before parking, so quick replies avoid a sleep and wake. Idle contexts use no CPU. For a context with `timeout:`, mini_racer starts a watchdog thread for every eval or call, while mini_racer_rs uses one watchdog thread per process. For the "Call in a timeout scope" rows, mini_racer measures a call on a context with `timeout:`, its only bound on one call, while mini_racer_rs measures a call on a plain context inside `MiniRacerRs.timeout`.

Startup plus loading Babel takes 13.20 ms against 44.29 ms, or 3.36x faster. Babel's 2.3 MB bundle compiles from the code cache.

## Memory

Each memory scenario runs in a fresh process. Resident memory is read after garbage collections, a pause, and one more collection, so released contexts have finished tearing down. Memory retained by malloc after a free still counts, as it does for an application. Per-context rows leave out the first context, which pays for starting V8. The ratio is how many times less memory mini_racer_rs uses, and a value above 1 favors mini_racer_rs.

| case | mini_racer 0.22.1 | mini_racer_rs | ratio | note |
|---|---:|---:|---:|---|
| RSS of a process with one idle context | 39.74 MB | 41.27 MB | 0.96x |  |
| RSS per idle context, 100 live | 1.39 MB | 1.58 MB | 0.88x |  |
| RSS per context with Babel loaded, 20 live | 28.51 MB | 15.03 MB | 1.90x |  |
| RSS per context from one Babel snapshot, 20 live | 17.37 MB | 9.21 MB | 1.89x |  |
| RSS per context with the MUI bundle loaded, 20 live | 110.37 MB | 43.49 MB | 2.54x |  |
| RSS growth per context over 20,000 created and disposed, after 2,000 | 1,671.17 bytes | 140.70 bytes | 11.88x | noisy |
| Peak RSS over 20,000 contexts left to the garbage collector | 2,909.91 MB | 105.56 MB | 27.57x |  |

Loaded contexts use less memory: Babel uses 15.03 MB against 28.51 MB, a context from one Babel snapshot uses 9.21 MB against 17.37 MB, and MUI uses 43.49 MB against 110.37 MB. An idle context costs 1.58 MB against 1.39 MB, or 0.88x. Contexts loading the same script of 1 MiB or more read one shared copy of its source, and contexts from one snapshot share its blob. Creating and disposing 20,000 contexts grows resident memory by 140.70 bytes per context against 1,671.17 bytes, although this row is noisy. When 20,000 contexts are left to Ruby's garbage collector, resident memory peaks at 105.56 MB against 2,909.91 MB. mini_racer_rs tells Ruby's garbage collector that each live context holds 2 MiB outside the Ruby heap with `rb_gc_adjust_memory_usage`, so collections run and finalize abandoned contexts sooner. mini_racer reports only the size of its context struct.

## mini_racer's own benchmark/serde suite

Times are microseconds per iteration. The suite's context has a 120 second timeout, so every case includes the cost of arming it.

| case | mini_racer 0.22.1 | mini_racer_rs | ratio | note |
|---|---:|---:|---:|---|
| `js_to_ruby/return_10k_ints` | 229.95 | 138.69 | 1.66x |  |
| `js_to_ruby/return_10k_floats` | 208.25 | 72.70 | 2.86x |  |
| `js_to_ruby/return_10k_ascii_strings` | 1671.57 | 644.57 | 2.59x |  |
| `js_to_ruby/return_10k_latin1_strings` | 5778.91 | 598.91 | 9.65x |  |
| `js_to_ruby/return_10k_utf8_bmp_strings` | 5863.22 | 617.25 | 9.50x | noisy |
| `js_to_ruby/return_10k_utf8_emoji_strings` | 5967.14 | 647.97 | 9.21x |  |
| `js_to_ruby/return_object_1k_ints` | 384.99 | 194.37 | 1.98x |  |
| `js_to_ruby/return_array_1k_objects` | 795.96 | 282.53 | 2.82x | noisy |
| `ruby_to_js/call_no_args` | 21.42 | 0.74 | 28.88x |  |
| `ruby_to_js/call_int_arg` | 21.84 | 0.77 | 28.25x |  |
| `ruby_to_js/call_short_string` | 22.47 | 1.01 | 22.22x | noisy |
| `ruby_to_js/call_utf8_string` | 23.98 | 1.08 | 22.19x |  |
| `ruby_to_js/call_emoji_string` | 24.12 | 1.07 | 22.45x |  |
| `ruby_to_js/roundtrip_10k_ints` | 564.21 | 291.65 | 1.93x | noisy |
| `ruby_to_js/roundtrip_10k_floats` | 620.97 | 303.03 | 2.05x | noisy |
| `ruby_to_js/roundtrip_10k_strings` | 2392.04 | 803.63 | 2.98x | noisy |
| `ruby_to_js/roundtrip_10k_utf8_bmp_strings` | 6438.18 | 973.82 | 6.61x |  |
| `ruby_to_js/roundtrip_10k_utf8_emoji_strings` | 6484.70 | 1045.40 | 6.20x | noisy |
| `ruby_to_js/roundtrip_hash_1k` | 351.93 | 164.88 | 2.13x | noisy |
| `ruby_to_js/roundtrip_array_1k_hashes` | 1195.88 | 521.73 | 2.29x |  |
| `callback/1000_echo_int` | 6096.02 | 1025.33 | 5.95x |  |
| `callback/1000_echo_short_string` | 6203.55 | 1139.69 | 5.44x |  |
| `callback/1000_echo_utf8_string` | 6390.16 | 1175.45 | 5.44x |  |
| `callback/1000_echo_emoji_string` | 6454.69 | 1184.53 | 5.45x |  |
| `callback/1000_add_ints` | 6154.60 | 1136.16 | 5.42x |  |

Every row favors mini_racer_rs, from 1.66x to 28.88x. Arrays of numbers decode from JavaScript to Ruby in runs of up to 64 elements, object keys are interned and each Hash is filled with one bulk insert, calls with few properties are built by the runner, and primitive results are written without V8's serializer.

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
