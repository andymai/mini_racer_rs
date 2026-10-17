# mini_racer_rs

Rust replacement for the `mini_racer` gem 0.22.1: same `MiniRacer` API, V8 from the `v8` crate (rusty_v8), Ruby bindings through magnus and rb-sys. The parity bar is mini_racer's own test suite, run unmodified.

## Hard rules

- Rust owns behavior. Ruby under `lib/` is vocabulary only: constants, the error hierarchy, `MiniRacer::Binary`, the loader, and the pure compositions `Context#load` and `#write_heap_snapshot`. The one exception is upstream's TruffleRuby backend, `lib/mini_racer/truffleruby.rb` and `lib/mini_racer/shared.rb`, vendored byte for byte and never edited.
- Intra-gem loads use `require_relative`.
- `unsafe` lives only in the modules `layers.toml` fences (`engine::sys`, `ext::rb_raw`, `ext::guard`, `ext::gc_root`), one operation per block, each with a `// SAFETY:` comment.
- rb-sys is called only from `ext/mini_racer_rs/src/rb_raw.rs`. Release the GVL only through `rb_raw::nogvl` and run interrupts only through `rb_raw::check_ints`, so no Ruby exception unwinds through a Rust frame.
- Ruby methods are defined only through `export!`; every `extern "C"` body calls `guard()` or `catch_unwind`.
- Ruby values kept in Rust storage go through `ext/mini_racer_rs/src/gc_root.rs`.
- Process-global engine state lives in `crates/mini_racer_engine/src/process.rs` and its submodules.
- Behavior that differs from mini_racer 0.22.1 is a deliberate deviation with a failing-first test and a recorded reason.
- No en or em dashes anywhere: code, comments, docs, commit messages. `scripts/check-prose.sh` enforces it.
- Comments explain a non-obvious why and nothing else.
- Never `--no-verify`, `HUSKY=0`, or a hook edit to get past a failing hook; `.claude/hooks/block-bypass.py` refuses them (a tripwire; CI is the gate).
- Public prose (README, docs, gemspec text, the repository description, anything filed upstream) goes through the `codex-write` skill and Andy's approval. PR titles and bodies go through codex-write and open once the fact-check and checker pass.

## Layers

`layers.toml` is the source of truth; `scripts/check-boundaries.sh` fails CI if this table drifts from it.

| Crate | Layer | Path | Role |
|---|---|---|---|
| `mini_racer_sync` | L0 | `crates/mini_racer_sync` | Concurrency protocol types; `forbid(unsafe_code)` |
| `mini_racer_wire` | L0 | `crates/mini_racer_wire` | V8 wire format codec; `forbid(unsafe_code)` |
| `mini_racer_engine` | L1 | `crates/mini_racer_engine` | V8 process state, flags, runners; the only crate that names `v8` |
| `mini_racer_rs` | L2 | `ext/mini_racer_rs` | The Ruby extension (cdylib); the only crate that names magnus or rb-sys |

## Invariants

- I1: isolate-scoped V8 calls run only on the thread that owns or locks that isolate. Other threads reach it only through `IsolateHandle` (terminate, cancel, interrupt).
- I2: an isolate's custody is on its runner's stack or deposited in its control block; a deposit with an idle channel is the only fork-recoverable state.
- I3: Ruby values live in Rust storage only through `gc_root`; the decoder holds them only in native stack locals.
- I4: every FFI entry point is panic-guarded.
- I5: each channel message is delivered exactly once.
- I6: no Ruby non-local exit crosses a Rust frame; jumps resume only after the Rust frames below the method entry have returned.
- I7: lock order is GVL, then context lock, then the engine's process locks; no engine lock is held while waiting for the GVL.
- I8: V8-owned objects are dropped on a runner thread of the current fork generation.

## Where things live

| Task | File |
|---|---|
| Flag normalization, single-threaded detection, stack size | `crates/mini_racer_engine/src/process/platform.rs` |
| V8 initialization (a custom platform, so foreground tasks come to the runners) and version | `crates/mini_racer_engine/src/process.rs` |
| Runner thread, script evaluation, attach and the callback trampoline, heap statistics and snapshots, the collection after `ensure_gc_after_idle:`, the verbose `TryCatch` for `verbose_exceptions:`, mini_racer's error formats | `crates/mini_racer_engine/src/runner.rs` |
| `eval_await` and `call_await` (`await_promise`), `pump_message_loop`, `perform_microtask_checkpoint`, the nested await refusal, the last termination check before a result serializes (`verdict`) | `crates/mini_racer_engine/src/runner.rs` |
| V8's foreground tasks: the platform hook, each process's queues by isolate, registering and retiring an isolate's queue | `crates/mini_racer_engine/src/process/tasks.rs`, `crates/mini_racer_engine/src/runner.rs` (`retire`) |
| One isolate's task queue: posting order, delays, non-nestable tasks, the wait a post or termination ends | `crates/mini_racer_sync/src/tasks.rs` |
| Sanitizing results, the filter's safe context, serializing with the filter retry | `crates/mini_racer_engine/src/serialize.rs` |
| Context handle, dispatch, nested dispatch from callbacks, the `Host` contract, stop, dispose | `crates/mini_racer_engine/src/context.rs` |
| Control block shared by a handle and its runner, request types, context config | `crates/mini_racer_engine/src/control.rs` |
| Timeout watchdog thread | `crates/mini_racer_engine/src/process/watchdog.rs` |
| The descriptor a fiber scheduler waits on for a context's runner | `crates/mini_racer_engine/src/notify.rs` |
| Forks: the fork epoch, per-process generations, quiescing runners inside `fork()` (`pthread_atfork`), taking idle contexts over in the child, dispatches orphaned by a fork from a callback | `crates/mini_racer_engine/src/process/fork.rs`, `crates/mini_racer_engine/src/sys.rs`, `crates/mini_racer_engine/src/context.rs` (`Lineage`, `adopt`) |
| `max_memory:`: the garbage collection callback and the heap check after it | `crates/mini_racer_engine/src/sys.rs` (`watch_heap`), `crates/mini_racer_engine/src/runner.rs` (`check_heap`) |
| Building and warming up startup snapshots | `crates/mini_racer_engine/src/snapshot.rs` |
| Engine errors | `crates/mini_racer_engine/src/error.rs` |
| `madvise` wrapper (fork safety), and the fixture showing a child crashes without it | `crates/mini_racer_engine/src/sys.rs`, `crates/mini_racer_engine/build.rs`, `scripts/madvise-fixture.sh`, `test/stress/madvise_fixture.rb` |
| Wire decoding, writing and text transcoding | `crates/mini_racer_wire/src/decode.rs`, `crates/mini_racer_wire/src/write.rs`, `crates/mini_racer_wire/src/text.rs` |
| The string that stands in for a function result | `crates/mini_racer_wire/src/marker.rs` |
| What V8 writes for a corpus of expressions, and its reviewed decoding | `crates/mini_racer_wire/tests/golden/`, `crates/mini_racer_engine/examples/gen_wire_golden.rs` |
| V8 reading what `Writer` writes | `crates/mini_racer_engine/tests/writer.rs` |
| Wire fuzz targets | `crates/mini_racer_wire/fuzz/fuzz_targets/`, `scripts/fuzz-smoke.sh` |
| Mutex poison policy | `crates/mini_racer_sync/src/lock.rs` |
| Turn-taking channel between a Ruby thread and a runner, parking, notifiers for waiters that sleep elsewhere | `crates/mini_racer_sync/src/channel.rs`, `crates/mini_racer_sync/src/parker.rs` |
| Termination causes, stop consumption, a stop whose one-shot V8 interrupt is spent (`spend`, `renew`, `relocked`), timeout scoping, and their tests against mini_racer's behavior | `crates/mini_racer_sync/src/termination.rs`, `test/ruby/pending_termination_test.rb`, `test/ruby/spent_termination_test.rb`, `test/ruby/leftover_microtasks_test.rb` |
| Runner phases and the gate that holds runners back during a fork | `crates/mini_racer_sync/src/fork.rs` |
| std or loom primitives for protocol code | `crates/mini_racer_sync/src/primitives.rs` |
| Loom models | `crates/mini_racer_sync/tests/loom.rs` |
| `MiniRacer::Context` methods, per-context lock | `ext/mini_racer_rs/src/context.rs` |
| Waiting from a Ruby thread (spin, fiber scheduler, GVL release, park, interrupts) and running attached callables | `ext/mini_racer_rs/src/host.rs` |
| Waiting for contexts from fibers under a scheduler (the `async` gem) | `test/ruby/fiber_scheduler_test.rb` |
| `Context.new` option rules (Ruby-free) | `ext/mini_racer_rs/src/options.rs` |
| `MiniRacer::Snapshot` | `ext/mini_racer_rs/src/snapshot.rs` |
| `MiniRacer::Platform.set_flags!` | `ext/mini_racer_rs/src/platform.rs` |
| JavaScript to Ruby values | `ext/mini_racer_rs/src/decode.rs` |
| Ruby to JavaScript values (`Context#call` arguments) | `ext/mini_racer_rs/src/encode.rs` |
| Ruby values held by Rust storage: class roots, `DateTime`, values pinned during an encode | `ext/mini_racer_rs/src/gc_root.rs` |
| Converting values under `GC.stress` with compaction | `test/ruby/conversion_gc_test.rb`, `test/upstream/gc_stress.txt`, `test/upstream/run.rb --gc-stress` |
| Error class mapping, binary messages for errors mini_racer raises from C, `ScriptError#cause` | `ext/mini_racer_rs/src/errors.rs` |
| GVL release, interrupts, raw rb-sys | `ext/mini_racer_rs/src/rb_raw.rs` |
| The `Process._fork` and `Process.daemon` hooks marking forks whose child runs Ruby | `ext/mini_racer_rs/src/fork.rs` |
| Forking with contexts, one process per case | `test/ruby/fork_test.rb`, `test/ruby/fork/` |
| Loader and conflict guard | `lib/mini_racer_rs.rb` |
| Error hierarchy and Ruby compositions | `lib/mini_racer_rs/vocabulary.rb` |
| Native build, link flags | `ext/mini_racer_rs/extconf.rb`, `ext/mini_racer_rs/build.rs` |
| Export allowlists | `ext/mini_racer_rs/exports.allow.linux`, `ext/mini_racer_rs/exports.allow.macos` |
| V8 archive pins | `ext/mini_racer_rs/v8-archives.sha256`, `scripts/fetch-v8-archive.sh`, `scripts/update-v8-archives.sh` |
| Third-party notices: Rust crates, the Rust standard library, and the code in the V8 archives | `THIRD_PARTY_NOTICES.md`, `about.toml`, `about.hbs`, `licenses/v8/`, `licenses/rust/`, `scripts/update-notices.sh` |
| Dependency updates and advisory scans | `.github/dependabot.yml`, `.github/workflows/dependabot-auto-merge.yml`, `.github/workflows/osv-scan.yml`, `osv-scanner.toml` |
| CI | `.github/workflows/ci.yml`, `.github/actions/setup/action.yml` |
| Releases: the release PR, native and source gem builds, install tests, publishing | `.github/workflows/release-please.yml`, `.github/workflows/release.yml`, `release-please-config.json`, `.release-please-manifest.json` |
| Gem packaging (one extension per Ruby minor in a native gem) and the installed-gem smoke test | `rakelib/package.rake`, `release/smoke.rb`, `tools/versions.toml` (`[release]`) |
| Release checks: glibc and macOS floors, gem sizes, publish order, lockfiles after a version bump | `scripts/check-glibc-floor.sh`, `scripts/check-macos-minos.sh`, `scripts/check-gem-sizes.sh`, `scripts/publish-gems.sh`, `scripts/sync-release-locks.sh` |
| Sanitizer runs (nightly, and on PRs labeled `sanitizers`) | `.github/workflows/nightly.yml`, `scripts/sanitizers.sh`, `.github/sanitizers/lsan.supp` |
| Nightly fuzzing (15 minutes per target) and fresh-seed differential runs (30 minutes) | `.github/workflows/nightly.yml`, `scripts/fuzz-smoke.sh`, `test/diff/run.rb` (`--explore`) |
| Weekly: Ruby head, the source gem with and without a system libclang, upstream's scripts including the crash script, the newest v8 crate | `.github/workflows/extended.yml` |
| Weekly benchmark on a hosted runner (informational) | `.github/workflows/bench.yml` |
| Engine teardown checked for leaks, in both V8 platform modes | `crates/mini_racer_engine/tests/teardown.rs`, `crates/mini_racer_engine/tests/teardown_single_threaded.rs` |
| Upstream suite runner, suites, guard, reporter | `test/upstream/run.rb`, `test/upstream/suites.rb`, `test/upstream/preload.rb`, `test/upstream/reporter.rb` |
| Per-test parity ledgers | `test/upstream/mini_racer.status.toml`, `test/upstream/execjs.status.toml`, `test/upstream/milestones.toml` |
| Ledger generation | `.github/workflows/ledger-generate.yml`, `rakelib/ledger.rake`, `test/upstream/milestone_rules.rb` |
| Upstream source (never edited) | `vendor/mini_racer` (v0.22.1), `vendor/execjs` (v2.10.2) |
| Differential run against the real gem | `test/diff/run.rb`, `test/diff/workers.rb`, `test/diff/worker.rb`, `test/diff/canonical.rb`, `test/diff/generate.rb`, `test/diff/shrink.rb`, `test/diff/ledger.rb`, `test/diff/shapes.rb` |
| Consumer workloads (Babel, Terser, Handlebars, React) and their ledger | `test/consumers/workloads.rb`, `test/consumers/js/`, `test/consumers/candidate.status.toml` |
| rtlcss resolving through the shim to mini_racer_rs | `test/consumers/transitive/` |
| Benchmarks against the real gem (informational), including mini_racer's own `benchmark/serde` suite | `bench/run.rb`, `bench/measure.rb`, `bench/payload.rb`, `bench/serde.rb` |
| Memory, lifecycle, fork and spawn stress runs (round trips and churn nightly, and on PRs labeled `stress`) | `test/stress/round_trips.rb`, `test/stress/churn.rb`, `test/stress/fork_loop.rb`, `test/stress/spawn_churn.rb`, `.github/workflows/nightly.yml` |
| Differential ledgers and corpus | `test/diff/candidate.status.toml`, `test/diff/divergences.toml`, `test/diff/corpus/` |
| Vendored TruffleRuby backend | `lib/mini_racer/truffleruby.rb`, `lib/mini_racer/shared.rb` |
| Public surface status per id, deviation reasons | `compat.toml`, `compat/surface.json` |
| Compat reflection, extraction, check and rendering | `tools/compat/`, `rakelib/compat.rake` |
| Generated from compat.toml (never edit) | `COMPATIBILITY.md`, the README compat table |
| Public docs (written through codex-write, approved by Andy) and GitHub templates | `README.md`, `ARCHITECTURE.md`, `SECURITY.md`, `STABILITY.md`, `CONTRIBUTING.md`, `CODE_OF_CONDUCT.md` (Contributor Covenant 2.1, verbatim), `.github/ISSUE_TEMPLATE/`, `.github/pull_request_template.md` |
| Weekly issue when RubyGems.org lists a newer mini_racer than the target | `.github/workflows/upstream-watch.yml`, `.github/upstream-release.md` |

Skills: `.claude/skills/pr-workflow` (branch to merge), `.claude/skills/layer-boundaries` (dependencies, fences, check failures), `.claude/skills/parity-suite` (upstream suite and ledger), `.claude/skills/release-flow` (releases, packaging, publishing), `.claude/skills/v8-bump` (moving the pinned V8).

## Commands

```bash
bin/setup                          # toolchains, pinned tools, gems, npm, V8 archive, compile
. scripts/fetch-v8-archive.sh      # export the verified V8 archive (or use .envrc.example)
scripts/gate.sh --quick            # static checks and clippy
scripts/gate.sh                    # plus Rust tests, compile, exports, Ruby tests
scripts/gate.sh --full             # plus planted violations, MSRV, deny, audit, machete, rustdoc, gem builds
bundle exec rake compile           # build the extension into lib/mini_racer_rs/
bundle exec rake test              # Ruby tests (compiles first)
cargo nextest run --workspace --exclude mini_racer_rs
scripts/check-exports.sh --write   # after changing #[no_mangle] items or bumping v8
scripts/update-v8-archives.sh      # repin every target's V8 archive after bumping v8
scripts/update-notices.sh          # regenerate THIRD_PARTY_NOTICES.md and licenses/v8 (--check to verify)
RUSTFLAGS="--cfg loom -Dwarnings" cargo test -p mini_racer_sync --test loom --release --locked --target-dir target/loom
scripts/fuzz-smoke.sh 60           # every wire fuzz target, 60 s each, on the pinned nightly
scripts/sanitizers.sh              # ASan teardown and memory checks, TSan on sync (Linux x86_64, needs llvm-symbolizer)
cargo run -p mini_racer_engine --example gen_wire_golden   # rewrite the golden .bin files from V8
UPDATE_GOLDEN=1 cargo test -p mini_racer_wire --test golden   # rewrite their .txt renderings, then review the diff
bundle exec ruby test/upstream/run.rb --strict   # mini_racer's own suite against the ledger
bundle exec ruby test/upstream/run.rb --suite execjs --strict   # ExecJS's suite on its MiniRacer runtime
bundle exec ruby test/upstream/run.rb --gc-stress --strict   # the passing tests in gc_stress.txt under GC.stress with compaction
bundle exec rake compat:check      # compat.toml against the reference surface and this gem
bundle exec rake compat:render     # regenerate everything compat.toml generates
bundle exec rake compat:verify     # compat:check plus a fresh reflection of the real gem (needs tools/reference)
bundle exec ruby test/diff/run.rb --twin --strict   # the real gem and this one on the same values (needs tools/reference)
bundle exec ruby test/diff/run.rb --set consumers --twin --strict   # real libraries (npm run build in test/consumers/js first)
bundle exec ruby test/diff/run.rb --explore --shrink --seed 42 --js 1500 --ruby 1500   # a fresh seed, no ledger, no shared objects
BUNDLE_GEMFILE=$PWD/test/consumers/transitive/Gemfile bundle exec ruby test/consumers/transitive/check.rb
bundle exec ruby bench/run.rb     # medians against the real gem; --quick for a smoke run
bundle exec ruby -Ilib test/stress/round_trips.rb multi   # 1M evals after a 500k warm-up; fails on RSS growth over 5% (also: single)
bundle exec ruby -Ilib test/stress/churn.rb multi 8 600   # 8 threads churning contexts for 10 min; fails over 256 bytes of RSS per context plus 32 MB (also: single)
bundle exec ruby -Ilib test/stress/fork_loop.rb    # 500 forks with idle contexts, 100 during an eval loop, single-threaded
bundle exec ruby -Ilib test/stress/spawn_churn.rb single   # 1,000 backticks and IO.popen under context churn (also: multi)
scripts/madvise-fixture.sh         # Linux: build without the madvise wrapper and show a fork child crash
bundle exec rake package:stage PLATFORM=x86_64-linux-gnu   # copy the compiled extension to tmp/native/<platform>/<ruby minor>/
bundle exec rake package:native PLATFORM=x86_64-linux-gnu  # a native gem from every staged Ruby's extension, into pkg/
bundle exec rake package:source    # the source gem, into pkg/
```

## Which tests to run

- `crates/mini_racer_wire`: `cargo nextest run -p mini_racer_wire`, then `scripts/fuzz-smoke.sh 20`. A new corpus entry goes in `crates/mini_racer_wire/tests/golden/corpus.tsv`; generate its `.bin`, then its `.txt`, and read the `.txt` before committing.
- `crates/mini_racer_sync`: `cargo nextest run -p mini_racer_sync`, then the loom command above.
- `crates/mini_racer_engine`: `cargo nextest run -p mini_racer_engine`; for teardown or lifetime changes, `scripts/sanitizers.sh` or the `sanitizers` label. Flag-dependent assertions live in `crates/mini_racer_engine/tests/flags.rs`, one test per process.
- `ext/` or `lib/`: `cargo test -p mini_racer_rs --lib` (the Ruby-free option rules and number classification), `bundle exec rake test`, then `scripts/check-exports.sh`, then `bundle exec ruby test/upstream/run.rb --strict` with and without `--suite execjs`, then `bundle exec ruby test/diff/run.rb --strict` with and without `--set consumers`, then `bundle exec rake compat:check`.
- `compat.toml`: `bundle exec rake compat:render`, then `bundle exec rake compat:check`.
- `scripts/`, `layers.toml`, any `Cargo.toml`: `scripts/check-boundaries.sh`, `scripts/test-boundaries.sh` and `scripts/check-deny.sh`.
- `.github/`: `scripts/check-action-pins.sh`.
- `Cargo.lock`, `rust-toolchain.toml`, `licenses/`, `about.*`: `scripts/update-notices.sh --check`.

## Pitfalls

- `lib/mini_racer_rs/version.rb` holds one version literal; release-please rewrites the first one it finds, and the Cargo workspace version. `scripts/sync-release-locks.sh`, which the release PR workflow runs, brings both `Gemfile.lock`s, `Cargo.lock` and the fuzz crate's `Cargo.lock` along; `scripts/check-versions.sh` fails until they agree.
- Loom builds need `--cfg loom` in `RUSTFLAGS` and their own `--target-dir target/loom`; sharing the default target dir rebuilds everything on every switch.
- The fuzz crate is its own workspace with its own `Cargo.lock` (checked by `cargo fetch --locked`, cargo deny and cargo audit); it builds only on the nightly pinned as `toolchains.fuzz` in `tools/versions.toml`, which `scripts/fuzz-smoke.sh` installs when missing.
- `rake compile` builds the dev profile locally and release when `CI` is set (`RB_SYS_CARGO_PROFILE`).
- A stale `lib/mini_racer_rs/mini_racer_rs.so` keeps loading after Rust edits until `rake compile` runs; `NATIVE_VERSION` only catches version mismatches.
- Without the variables from `scripts/fetch-v8-archive.sh`, cargo downloads V8 itself and skips the pinned digest.
- A dependency, toolchain or v8 change regenerates `THIRD_PARTY_NOTICES.md` (`scripts/update-notices.sh`); it downloads every pinned V8 archive and fails if one links source that `licenses/v8/components.tsv` does not list.
- cargo-about installed from source needs `--features cli`; `bin/setup` passes it.
- V8's platform and flags are process-global and freeze at first use, so tests that set flags run in their own process.
- `mini_racer_rs.gemspec` lists files by explicit globs; a new file outside them is missing from the gem.
- Source builds need libclang; with the `libclang` gem, `ext/mini_racer_rs/extconf.rb` also passes GCC's include directory to bindgen.
- Each git worktree needs its own `npm ci` before the commit hooks can run, and `git submodule update --init` before the upstream suites or `rake test` (the ExecJS autodetect test) can run.
- The upstream suites run from their submodule (`vendor/mini_racer` or `vendor/execjs`) with `BUNDLE_GEMFILE`, `RUBYOPT`, `MINI_RACER_RS_UPSTREAM_REPORT`, `MINI_RACER_RS_UPSTREAM_ROOT` and, for ExecJS, `EXECJS_RUNTIME` set by `test/upstream/run.rb`; run them through that script, not with `rake test` inside the submodule.
- A forked child takes over a single-threaded context only if its runner was idle at a fork that went through `Process._fork` or `Process.daemon`; every other inherited context raises `MiniRacer::RuntimeError` (`single-threaded runner: ...`). The child never frees or waits on anything inherited: it reads atomics and try-locks only. Processes are told apart by `process::fork::epoch()`, never by pid.
- A `compat.toml` status describes CRuby. Implementing a planned method without marking it `done` fails `compat:check` with `TODO_BUT_IMPLEMENTED`; a planned method that exists before what it calls works (`write_heap_snapshot`) lists that in `depends_on`.
- `rake` needs the lint group installed; without it (the TruffleRuby CI job), run `tools/compat/reflect.rb` and `tools/compat/check.rb` directly.
- mini_racer 0.22.1 aborts the whole process (signal 6) on some values, such as a `Map` with a function key; the differential run records that as a `crash` outcome, and worker stderr is in `tmp/diff/<role>-worker.log`. A worker that exits with a status instead is a harness failure and stops the run.
- Plain V8 has no `MessageChannel` or `TextEncoder`, which React's server renderer reaches for when it loads; `test/consumers/js/entries/ssr-globals.js` provides the stand-ins SSR setups on mini_racer use. Bundles are built into `tmp/consumers/bundles` by `npm run build` in `test/consumers/js`.
