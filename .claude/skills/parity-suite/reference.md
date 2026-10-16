# Parity Suite reference

## How a run is wired

`test/upstream/run.rb` starts one Ruby process from the suite's directory (`test/upstream/suites.rb`) that requires its test files, as upstream's Rake task does: for mini_racer every `test/*_test.rb` plus `test/test_object_identity_fix.rb` from `vendor/mini_racer` with `test` on the load path; for ExecJS `test/test_execjs.rb` from `vendor/execjs` with `lib` and `test` on the load path and `EXECJS_RUNTIME=MiniRacer`. Both run with:

- `BUNDLE_GEMFILE` set to this repository's `Gemfile` (or `tools/reference/Gemfile`), because upstream's `test_forking` runs `bundle exec` from its own directory;
- `RUBYOPT` carrying `-rbundler/setup` and `test/upstream/preload.rb` (or `test/upstream/preload_reference.rb`), so every Ruby process the tests spawn loads the preload too;
- `MINI_RACER_RS_UPSTREAM_REPORT`, where `test/upstream/reporter.rb` writes a JSON line when each test starts and finishes, and `MINI_RACER_RS_UPSTREAM_ROOT`, the suite directory test ids are relative to.

The preload puts this gem's `lib/` first, requires `mini_racer`, and exits 97 if any file under upstream's `lib/` is required, loaded or compiled, or if upstream's native extension appears in `$LOADED_FEATURES` at exit. `test/ruby/harness_guard_test.rb` proves that against a fake upstream directory.

## Watchdog

The runner reads the events while the suite runs. A test that exceeds its limit (120 seconds, or `timeout_s` from its ledger entry) gets USR2 (the reporter writes every Ruby thread's backtrace), then a native stack capture (`eu-stack` on Linux, `sample` on macOS), then SIGKILL. The runner records `timeout` for that test, a test in flight when the process dies on its own as `crash`, and restarts the suite excluding every test that already has a result.

## Initial milestones

`test/upstream/milestone_rules.rb` assigns a failing test to the first rule that matches: tests named in a spike step's exit criterion go to that step (S2, S3, S3b, S6); then ExecJS's `test/test_execjs.rb` to M8; then `test/single_threaded_test.rb` and forking tests to M6; `test/async_test.rb` and wasm, promise or microtask tests to M4; `test/function_test.rb` and `test/test_object_identity_fix.rb` to M2; snapshot, warmup and flag tests to M5; memory, heap, dispose, stop, timeout, termination, GC and idle tests to M3; everything else to M1.

## Scripts

`test/upstream/scripts.status.toml` lists upstream's standalone scripts with their expected exit, timeout and milestone. Until that milestone is marked `done` in `test/upstream/milestones.toml` a script's outcome is printed and written to `tmp/upstream/scripts-<leg>.jsonl` (or `--out`); afterwards a wrong exit fails the run. `test/test_crash.rb` is weekly and runs only with `--weekly`.

## Vendored files

`lib/mini_racer/truffleruby.rb` and `lib/mini_racer/shared.rb` are upstream's TruffleRuby backend, copied byte for byte, with upstream's license in `licenses/mini_racer.txt`. `scripts/check-vendored.sh` compares all three with `vendor/mini_racer`.
