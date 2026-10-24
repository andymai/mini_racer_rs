# Debugging reference

## Sanitizer reports

`scripts/sanitizers.sh` (Linux x86_64) builds with the nightly pinned as `toolchains.fuzz` in `tools/versions.toml`, sources `scripts/fetch-v8-archive.sh`, and needs `llvm-symbolizer` on `PATH` or as `/usr/bin/llvm-symbolizer-<n>`; without one it stops with `sanitizers: llvm-symbolizer not found` (the nightly job installs Ubuntu's `llvm` package when neither exists). It runs three passes and stops at the first that fails:

1. AddressSanitizer with leak detection on `crates/mini_racer_engine/tests/teardown.rs` and `crates/mini_racer_engine/tests/teardown_single_threaded.rs`, suppressions from `.github/sanitizers/lsan.supp`, in `target/asan`.
2. AddressSanitizer without leak detection on the lib and integration tests of `mini_racer_engine` and `mini_racer_sync`. Only the teardown tests check leaks: a dropped handle shuts its runner down asynchronously, so other test processes can exit with isolates alive.
3. ThreadSanitizer with `-Zbuild-std` and `halt_on_error=1` on `mini_racer_sync`'s unit tests, in `target/tsan`.

The report is on the test's stderr; the nightly Sanitizers job uploads nothing, so read it with `gh run view <run-id> --log-failed`. To rerun one binary or test with the first pass's settings:

```bash
. scripts/fetch-v8-archive.sh
toolchain=$(taplo get -f tools/versions.toml toolchains.fuzz)
ASAN_SYMBOLIZER_PATH=$(command -v llvm-symbolizer) ASAN_OPTIONS=detect_leaks=1 \
  LSAN_OPTIONS="suppressions=$PWD/.github/sanitizers/lsan.supp" RUSTFLAGS="-Zsanitizer=address" \
  cargo "+$toolchain" test --locked --target x86_64-unknown-linux-gnu --target-dir target/asan \
  -p mini_racer_engine --test teardown -- <test name>
```

rusty_v8 152.2.0 leaks a 64-byte liveness cell per isolate on purpose (`Box::leak` in `IsolateAnnex::new`), denoland/rusty_v8#2075. The suppression matches the function `<v8::isolate::Isolate>::create_annex` by name, so the same leak shows up as a failure when stacks are not symbolized, or after a v8 bump renames `create_annex` or stops inlining `IsolateAnnex::new` into it. Read the new version's `~/.cargo/registry/src/*/v8-<version>/src/isolate.rs` before editing `.github/sanitizers/lsan.supp`, and keep its comment naming the version. A leak whose stack names any other frame is a real one. Use-after-free and data race reports point at the frame to fix; for a race in `mini_racer_sync`, a loom model of the same interleaving is the regression test.

## Loom failures

The models run with `RUSTFLAGS="--cfg loom -Dwarnings" cargo test -p mini_racer_sync --test loom --release --locked --target-dir target/loom` (CI's Loom job and `scripts/gate.sh`); `--cfg loom` builds need their own target directory, since sharing the default one rebuilds everything on every switch. Append `-- <model name>` to run one. Loom explores interleavings in a fixed order, so rerunning the model replays the failing one; there is no seed to save.

The workspace pins loom 0.7.2 (`Cargo.lock`; its source is under `~/.cargo/registry/src/`). Its environment variables, as they apply here:

- `LOOM_LOG=trace` with `-- <model> --nocapture` prints every branch, thread switch and `Arc` count. It works only for models that call `loom::model`, which installs loom's tracing subscriber; `bounded` calls `Builder::check` directly and prints nothing.
- `LOOM_LOCATION=1` adds source locations to loom's panic messages. It is slow, so narrow to one model first.
- `LOOM_MAX_PREEMPTIONS=<n>` bounds the `loom::model` models. `bounded(<n>, ...)` sets its bound in code and ignores the variable.
- `LOOM_MAX_PERMUTATIONS` and `LOOM_MAX_DURATION` (seconds) stop the search early; loom checks them every 20,000 iterations.
- `LOOM_CHECKPOINT_FILE` needs loom's `checkpoint` feature, which the workspace does not enable.

To narrow a `bounded` model, lower its preemption bound locally until it passes; the smallest failing bound gives the shortest trace. To trace it, switch it locally to `loom::model` and pass that bound as `LOOM_MAX_PREEMPTIONS`, then add `LOOM_LOG=trace`. Revert both before committing.

## Fuzz crashes

`scripts/fuzz-smoke.sh <seconds>` runs each target in `crates/mini_racer_wire/fuzz/fuzz_targets/` (`decode_arbitrary`, `text_roundtrip`, `sourcemap`) for that long on the pinned nightly, with AddressSanitizer on, and stops at the first crash. libFuzzer saves the input under `crates/mini_racer_wire/fuzz/artifacts/<target>/` (`crash-<hash>`, or `leak-`, `timeout-`, `oom-`). CI's Fuzz Smoke job and the nightly Fuzz job upload that directory on failure: `gh run download <run-id> -n fuzz-artifacts`.

```bash
toolchain=$(taplo get -f tools/versions.toml toolchains.fuzz)
host=$(rustc "+$toolchain" -vV | sed -n 's/^host: //p')
cd crates/mini_racer_wire
cargo "+$toolchain" fuzz run --target "$host" <target> fuzz/artifacts/<target>/<input>    # replay once
cargo "+$toolchain" fuzz tmin --target "$host" <target> fuzz/artifacts/<target>/<input>   # minimize
cargo "+$toolchain" fuzz fmt --target "$host" <target> <input>                              # print the input's Debug form, a byte array
```

`--target "$host"` matters: a prebuilt cargo-fuzz defaults to its own triple, which can be musl. Keep the minimized bytes as a test in `crates/mini_racer_wire/tests/edges.rs`, whose `frame` and `body` helpers take raw bytes.

## Upstream hangs and crashes

`test/upstream/run.rb` runs the suite in one child process and follows `test/upstream/reporter.rb`'s event file, one JSON line when each test starts and finishes. A test past its limit (`timeout_s` from its ledger entry, else 120 seconds) gets USR2, on which the reporter writes every Ruby thread's backtrace; two seconds later the runner captures native stacks (`eu-stack -p`, or `sample` on macOS; the dump says so when neither is installed), then kills the process group. Both land in `tmp/upstream/<test id>.timeout.txt`, with each `/` and `#` of the id replaced by `_`. With no test in flight for 300 seconds the process counts as hung, and the dump is `tmp/upstream/<suite>-suite-attempt-<n>.timeout.txt`.

After a timeout, or a death inside a test (recorded as `crash`), run.rb starts another attempt that excludes every test with a result, so one bad test costs one result. Each attempt writes `tmp/upstream/<suite>-<mode>-<leg>-<seed>-<attempt>.log` and `.events.jsonl` beside it; results go to `tmp/upstream/<suite>-<mode>-<leg>-<seed>.jsonl`. A death outside any test is a suite failure, and an attempt that records nothing new aborts with the last 40 lines of its log. Exit 97 is the harness guard (parity-suite).

Reproduce with the seed from the summary line or the file names: `bundle exec ruby test/upstream/run.rb -n <test_name> --seed <seed>`, plus `--suite execjs` for ExecJS. Add `--reference` to see whether the real gem behaves the same (needs the `tools/reference` bundle). A hang the real gem shares becomes `upstream-fail` with a `cause`; one of ours that waits on a milestone becomes `crash:<M>`. Either way set `timeout_s`, so later runs stay short.

## Differential divergences

`test/diff/run.rb` feeds each case to a long-lived worker per side (`test/diff/workers.rb`) and compares step outcomes in canonical form. Results are `tmp/diff/values-<seed>.jsonl` (seed 1 by default) or `tmp/diff/consumers.jsonl`, one line per case with its `class` and the `expected` (real gem) and `actual` (this gem) outcomes. Worker stderr goes to `tmp/diff/reference-worker.log`, `tmp/diff/candidate-worker.log` and, with `--twin`, `tmp/diff/reference-twin-worker.log`.

- A worker killed by a signal records `["crash","signal <n>"]` for that step and restarts. mini_racer 0.22.1 aborted with signal 6 on some values, such as a `Map` with a function key, which 0.22.2 returns as an error Hash; if this gem returns something sensible where mini_racer crashes, the difference is accepted only through a `test/diff/divergences.toml` entry naming a `compat.toml` deviation.
- A worker that exits with a status is the harness failing (a Ruby exception escaped `test/diff/worker.rb`): the run stops with `the <role> worker exited with <n>; see <log>`.
- A case over 30 seconds records `["timeout",30]` and restarts the worker.
- `--shrink` minimizes each new divergence not already `xfail:` or `crash:` into `test/diff/corpus/min-<id>.json`, keeping how its first differing step differs.

The nightly explore job prints `values, seed <n>` per seed and uploads `tmp/diff` and the minimized cases as `diff-explore`. Replay one seed with `bundle exec ruby test/diff/run.rb --explore --shrink --seed <n> --js 1500 --ruby 1500`. A bug: fix it and commit the minimized case as a regression. A deliberate difference: a deviation, an accepted divergence, and `--write-ledger`.

## Fork cases

Each `test/ruby/fork/<what>_case.rb` runs as its own process; ForkTest kills its process group after 60 seconds and prints the status, stdout and stderr. Inside, `child` reports `child: <class>: <message>` (the block raised, exit 1) or `child timed out` (exit 99 after five seconds), and `reap` raises `child <pid> hung`. Run one directly with `bundle exec ruby test/ruby/fork/<what>_case.rb; echo $?` (0 passes, 77 skips, and with `CI` set a skip exits 1) or through Minitest with `bundle exec ruby -Itest/ruby test/ruby/fork_test.rb -n test_<what>`. While it hangs, `eu-stack -p <pid>` on the parent and on the child shows where each is blocked; a child runs V8's worker tasks on threads of its own, started at its first worker task, whose idle stacks wait in `WorkerQueue::take`. A child that aborts with Abseil's `Mutex::Fer with pending CondVar queueing`, or hangs on worker work such as an awaited `WebAssembly.compile`, posted to V8's own worker threads, which it does not have: a worker slot no longer leads to `process::workers` (v8-bump skill, step 11). The fork rules the engine follows are in `ARCHITECTURE.md` "Forks" and `.claude/skills/engine-internals/forks.md`; `test/stress/fork_loop.rb` forks 600 times with idle and busy contexts.

## Missing V8 static library

v8's build script unpacks the archive into `<target>/<profile>/gn_out/obj`, outside its own build directory, and rust-cache keeps the build script's recorded output but not that directory, so a CI change that rebuilds the v8 crate without rerunning its build script (a feature or version change in a crate below v8) fails with `could not find native static library rusty_v8` unless `scripts/restore-v8-static-lib.sh`, which the setup action runs after the cache, has put the library back; the newest-v8 canary in `extended.yml`, which skips the setup action's V8 steps, cleans v8 instead. Locally a missing `gn_out` fails the same way once the v8 crate is rebuilt; run the script, or `cargo clean -p v8` (with `--release` for a release build).

```bash
. scripts/fetch-v8-archive.sh && scripts/restore-v8-static-lib.sh
```

## Scheduled run issues

`.github/workflows/scheduled-failures.yml` watches Nightly, Extended, Bench, OSV Scan and Upstream Watch. When a run started by `schedule` ends as failure, timed_out, startup_failure or cancelled (a job past its timeout ends as cancelled), `scripts/report-scheduled-failure.sh` opens `Scheduled <workflow> run failed` with the run link, commit and failed jobs, or comments on the open issue with that title.

1. `gh run view <run-id> --log-failed` for the failing step.
2. Artifacts, where the job keeps them: `gh run download <run-id> -n <name>` for `fuzz-artifacts` and `diff-explore` (Nightly), `upstream-scripts` (Extended), `bench` (Bench). Sanitizers and Stress keep only logs.
3. Reproduce locally with the recipe above. Nightly's Sanitizers and Stress jobs also run on a PR labeled `sanitizers` or `stress`. `gh workflow run <workflow>` reruns Nightly, Extended, Bench or Upstream Watch (OSV Scan has no manual trigger); a dispatched run never opens an issue.
4. Close the issue once the fix is on `main`; while it stays open, each later failure comments on it.

`RUN_ID=<id> WORKFLOW=<name> scripts/report-scheduled-failure.sh --dry-run` prints what the report would post.
