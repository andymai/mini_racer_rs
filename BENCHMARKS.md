# Benchmarks

## Method

Each run compares mini_racer_rs with mini_racer 0.22.1 using libv8-node 24.12.0.1. Both run on Ruby 3.4.2 on Linux with an AMD Ryzen 9 7950X3D, the CPU governor set to `performance`, and both processes pinned to the same eight cores with `taskset`. The gems run in separate, alternating processes. Tables report medians across runs, and the ratio shows how many times faster mini_racer_rs is. A ratio above 1 means mini_racer_rs is faster.

Workloads use a release build at commit `bc492d3` with 9 runs. Per-call and serde results use a release build at commit `8ba3843` with 5 runs. The engine code is identical because `bc492d3` adds documentation only. The harness verifies identical output for every workload.

A row is marked `noisy` when either gem's results spread by more than 10% across runs. Treat the ratio on those rows as less precise. Absolute times depend on the machine, while ratios generally carry over better.

## Workloads

| case | mini_racer 0.22.1 | mini_racer_rs | ratio | note |
|---|---:|---:|---:|---|
| Babel transform of a 3 KB JSX component | 9.92 ms | 5.17 ms | 1.92x | |
| Handlebars render of 50 table rows | 49.49 µs | 41.92 µs | 1.18x | |
| React server render of 1,000 list items | 193.44 µs | 189.24 µs | 1.02x | noisy |
| Terser minify of a 78 KB library | 52.16 ms | 51.47 ms | 1.01x | |
| React in a new context, load bundle and render 1,000 items | 11.88 ms | 11.85 ms | 1.00x | |

Each workload runs in its own process. Babel warms up for 50 transforms before 160 timed transforms because V8 continues optimizing it that long. Babel, Terser, and React use ExecJS, matching Sprockets and react-rails. ExecJS removes `console` before loading a source, so the harness supplies Babel with a no-op `console`.

Babel's JavaScript takes 5.3 ms per transform inside mini_racer_rs on V8 15.2 and 10.3 ms inside mini_racer on V8 13.6. Its improvement comes from V8 execution, not call overhead. Terser and React spend nearly all their time in JavaScript and perform similarly. For the new-context workload, ExecJS contexts have no dispose and the two gems report their native memory to Ruby's garbage collector differently, so the harness collects garbage after each sample, outside the timing.

## Per-call costs, startup and payloads

| case | mini_racer 0.22.1 | mini_racer_rs | ratio | note |
|---|---:|---:|---:|---|
| eval round trip p50 | 6.59 µs | 1.26 µs | 5.22x | |
| eval round trip p99 | 10.58 µs | 2.92 µs | 3.62x | noisy |
| eval round trip p99.9 | 60.43 µs | 14.84 µs | 4.07x | noisy |
| call round trip p50 | 6.44 µs | 1.55 µs | 4.15x | |
| call round trip p99 | 10.52 µs | 2.42 µs | 4.34x | noisy |
| call round trip p99.9 | 79.14 µs | 13.59 µs | 5.82x | noisy |
| call on a context with `timeout:` p50 | 20.65 µs | 1.56 µs | 13.21x | |
| call on a context with `timeout:` p99 | 42.45 µs | 2.28 µs | 18.59x | noisy |
| callback round trip p50 | 6.10 µs | 1.28 µs | 4.76x | |
| context startup (`new`, `eval`, `dispose`) p50 | 1.54 ms | 1.09 ms | 1.41x | |
| startup and load Babel p50 | 44.54 ms | 40.26 ms | 1.11x | |
| startup from a Babel snapshot p50 | 10.82 ms | 5.05 ms | 2.14x | |
| Babel snapshot size | 5.68 MB | 5.76 MB | 0.99x | |
| 1 MB of JSON-like data, JavaScript to Ruby | 52.44 MB/s | 115.04 MB/s | 2.19x | |
| 1 MB of JSON-like data, Ruby to JavaScript | 185.27 MB/s | 200.01 MB/s | 1.08x | noisy |
| idle CPU of 50 contexts | 0% of a core | 0% of a core | n/a | |

mini_racer releases the GVL and passes each request and reply through a mutex and condition variable. mini_racer_rs briefly spins its runner and Ruby thread before parking, allowing quick replies without a sleep and wake. Idle contexts consume no CPU.

For a context with `timeout:`, mini_racer starts a watchdog thread for every eval or call. mini_racer_rs uses one watchdog thread per process.

## mini_racer's own benchmark/serde suite

Times are microseconds per iteration. The suite's context has a 120 second timeout, so every case includes the cost of arming it.

| case | mini_racer 0.22.1 | mini_racer_rs | ratio | note |
|---|---:|---:|---:|---|
| `js_to_ruby/return_10k_ints` | 233.49 | 260.01 | 0.90x | noisy |
| `js_to_ruby/return_10k_floats` | 205.93 | 200.12 | 1.03x | noisy |
| `js_to_ruby/return_10k_ascii_strings` | 1660.76 | 638.51 | 2.60x | |
| `js_to_ruby/return_10k_latin1_strings` | 5786.27 | 687.57 | 8.42x | |
| `js_to_ruby/return_10k_utf8_bmp_strings` | 5865.69 | 718.33 | 8.17x | noisy |
| `js_to_ruby/return_10k_utf8_emoji_strings` | 5953.19 | 780.32 | 7.63x | |
| `js_to_ruby/return_object_1k_ints` | 393.22 | 214.34 | 1.83x | noisy |
| `js_to_ruby/return_array_1k_objects` | 787.77 | 398.15 | 1.98x | |
| `ruby_to_js/call_no_args` | 21.78 | 1.54 | 14.17x | noisy |
| `ruby_to_js/call_int_arg` | 21.97 | 1.61 | 13.61x | noisy |
| `ruby_to_js/call_short_string` | 22.68 | 1.76 | 12.88x | |
| `ruby_to_js/call_utf8_string` | 24.48 | 1.82 | 13.47x | |
| `ruby_to_js/call_emoji_string` | 24.31 | 1.83 | 13.26x | |
| `ruby_to_js/roundtrip_10k_ints` | 568.94 | 729.57 | 0.78x | noisy |
| `ruby_to_js/roundtrip_10k_floats` | 615.73 | 721.69 | 0.85x | |
| `ruby_to_js/roundtrip_10k_strings` | 2338.54 | 1199.23 | 1.95x | |
| `ruby_to_js/roundtrip_10k_utf8_bmp_strings` | 6415.77 | 1392.96 | 4.61x | noisy |
| `ruby_to_js/roundtrip_10k_utf8_emoji_strings` | 6509.32 | 1469.07 | 4.43x | |
| `ruby_to_js/roundtrip_hash_1k` | 343.69 | 192.14 | 1.79x | |
| `ruby_to_js/roundtrip_array_1k_hashes` | 1154.05 | 713.39 | 1.62x | |
| `callback/1000_echo_int` | 6011.23 | 1186.32 | 5.07x | |
| `callback/1000_echo_short_string` | 6131.77 | 1280.59 | 4.79x | |
| `callback/1000_echo_utf8_string` | 6302.95 | 1341.17 | 4.70x | noisy |
| `callback/1000_echo_emoji_string` | 6301.74 | 1332.43 | 4.73x | |
| `callback/1000_add_ints` | 6223.95 | 1294.96 | 4.81x | |

Rows below 1x use arrays of 10,000 numbers. V8's serializer takes a slow path for those arrays in both gems.

## Reproducing

Run these commands from a checkout. Install mini_racer 0.22.1 and its dependencies under `tools/reference`:

```sh
cd tools/reference
BUNDLE_PATH=vendor/bundle bundle install
```

Build the Babel, Terser, React, and Handlebars consumer bundles:

```sh
cd test/consumers/js
npm ci
npm run build
```

Build the mini_racer_rs release extension:

```sh
CI=1 bundle exec rake compile
```

Run every table 5 times with both gems pinned to CPUs 0 through 7:

```sh
bundle exec ruby bench/run.rb --cpus 0-7
```

Use `--runs N` to change the run count, or `--quick` for one short smoke-test run of each case:

```sh
bundle exec ruby bench/run.rb --cpus 0-7 --runs N
bundle exec ruby bench/run.rb --cpus 0-7 --quick
```

Skip selected groups with `--no-micro`, `--no-serde`, or `--no-workloads`:

```sh
bundle exec ruby bench/run.rb --cpus 0-7 --no-micro
```

Workloads require current consumer bundles. The runner reports the required build command when they are missing or stale. Results are written to `tmp/bench/<timestamp>.json`.