# mini_racer_rs

Rust replacement for the `mini_racer` gem 0.22.2: same `MiniRacer` API, V8 from the `v8` crate (rusty_v8), Ruby bindings through magnus and rb-sys. The parity bar is mini_racer's own test suite, run unmodified.

## Hard rules

- Rust owns behavior. Ruby under `lib/` is vocabulary only: constants, the error hierarchy, `MiniRacer::Binary`, the loader, and the pure compositions `Context#load` and `#write_heap_snapshot`. The one exception is upstream's TruffleRuby backend, `lib/mini_racer/truffleruby.rb` and `lib/mini_racer/shared.rb`, vendored byte for byte and never edited.
- Intra-gem loads use `require_relative`.
- `unsafe` lives only in the modules `layers.toml` fences (`engine::sys`, `ext::rb_raw`, `ext::guard`, `ext::gc_root`), one operation per block, each with a `// SAFETY:` comment.
- rb-sys is called only from `ext/mini_racer_rs/src/rb_raw.rs`. Release the GVL only through `rb_raw::nogvl` and run interrupts only through `rb_raw::check_ints`, so no Ruby exception unwinds through a Rust frame.
- Ruby methods are defined only through `export!`; every `extern "C"` body calls `guard()` or `catch_unwind`.
- Ruby values kept in Rust storage go through `ext/mini_racer_rs/src/gc_root.rs`.
- Process-global engine state lives in `crates/mini_racer_engine/src/process.rs` and its submodules.
- Behavior that differs from mini_racer 0.22.2 is a deliberate deviation with a failing-first test and a recorded reason.
- A new fast-call shape lands only with tests pinning its outcomes to mini_racer 0.22.2.
- Every `terminate_execution` goes through `IsolateTerminator` or `runner::terminate`, which set the termination flag before V8 is asked to terminate; it is cleared only once V8 has cancelled.
- No en or em dashes anywhere: code, comments, docs, commit messages. `scripts/check-prose.sh` enforces it.
- Comments explain a non-obvious why and nothing else.
- Never `--no-verify`, `HUSKY=0`, or a hook edit to get past a failing hook; `.claude/hooks/block-bypass.py` refuses them (a tripwire; CI is the gate).
- Public prose (README, docs, gemspec text, the repository description, anything filed upstream) goes through the `outward-writing` skill and Andy's approval. PR titles and bodies go through outward-writing and open once the fact-check and checker pass.

## Layers

`layers.toml` is the source of truth; `scripts/check-boundaries.sh` fails CI if this table drifts from it.

| Crate | Layer | Path | Role |
|---|---|---|---|
| `mini_racer_sourcemap` | L0 | `crates/mini_racer_sourcemap` | Source maps for stack traces: parsing, lookup, rewriting frame locations; `forbid(unsafe_code)` |
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
- I8: V8-owned objects are dropped on a thread of the current fork generation: a runner, or for a worker task, a thread of that generation's worker pool.

## Where things live

| Task | File |
|---|---|
| V8 flags, single-threaded detection, stack size | `crates/mini_racer_engine/src/process/platform.rs` |
| V8 initialization, its custom platform, version | `crates/mini_racer_engine/src/process.rs` |
| Runner thread, evaluation, callbacks, heap stats, error formats | `crates/mini_racer_engine/src/runner.rs` |
| Code cache for evals of 128 KiB or more, and `MiniRacerRs.code_cache` | `crates/mini_racer_engine/src/process/code_cache.rs`, `crates/mini_racer_engine/src/runner.rs`, `ext/mini_racer_rs/src/counters.rs` |
| One shared copy of each large source | `crates/mini_racer_engine/src/process/sources.rs`, `crates/mini_racer_engine/src/text.rs`, `crates/mini_racer_engine/src/sys.rs` |
| Fast calls for ExecJS calls and react-rails renders | `crates/mini_racer_engine/src/fast_calls.rs`, `crates/mini_racer_engine/src/runner.rs` |
| Freed malloc memory returned after large evals and snapshots | `crates/mini_racer_engine/src/runner.rs`, `crates/mini_racer_engine/src/sys.rs` |
| Awaits, the message pump, microtask checkpoints | `crates/mini_racer_engine/src/runner.rs` |
| V8's foreground tasks, queues by isolate | `crates/mini_racer_engine/src/process/tasks.rs` |
| V8's worker tasks and the engine's pool | `crates/mini_racer_engine/src/sys.rs`, `crates/mini_racer_engine/src/process/workers.rs` |
| One isolate's task queue | `crates/mini_racer_sync/src/tasks.rs` |
| One process's worker task queue | `crates/mini_racer_sync/src/workers.rs` |
| Sanitizing and serializing results | `crates/mini_racer_engine/src/serialize.rs` |
| Primitive results written without V8's serializer | `crates/mini_racer_engine/src/serialize.rs`, `crates/mini_racer_engine/src/sys.rs` |
| A call's arguments built by the runner | `crates/mini_racer_engine/src/request.rs`, `crates/mini_racer_engine/src/runner.rs` |
| Context handle, dispatch, the `Host` contract | `crates/mini_racer_engine/src/context.rs` |
| Control block, request types, context config | `crates/mini_racer_engine/src/control.rs` |
| Timeout watchdog thread | `crates/mini_racer_engine/src/process/watchdog.rs` |
| `MiniRacerRs.timeout` scopes | `ext/mini_racer_rs/src/timeout.rs`, `crates/mini_racer_engine/src/control.rs`, `crates/mini_racer_sync/src/termination.rs` |
| The descriptor a fiber scheduler waits on | `crates/mini_racer_engine/src/notify.rs` |
| Fork epoch, generations, quiescing, takeover | `crates/mini_racer_engine/src/process/fork.rs`, `crates/mini_racer_engine/src/context.rs` |
| `max_memory:` collection callback and heap check | `crates/mini_racer_engine/src/sys.rs`, `crates/mini_racer_engine/src/runner.rs` |
| `MiniRacerRs.heap_limit` and `HeapLimitError` | `crates/mini_racer_engine/src/process/platform.rs`, `crates/mini_racer_engine/src/runner.rs`, `ext/mini_racer_rs/src/heap_limit.rs`, `lib/mini_racer_rs/heap_limit_error.rb` |
| Startup snapshots and their shared blob | `crates/mini_racer_engine/src/snapshot.rs`, `crates/mini_racer_engine/src/sys.rs` |
| Engine errors | `crates/mini_racer_engine/src/error.rs` |
| `madvise` wrapper for fork safety | `crates/mini_racer_engine/src/sys.rs`, `crates/mini_racer_engine/build.rs`, `scripts/madvise-fixture.sh` |
| Wire decoding, writing and text transcoding | `crates/mini_racer_wire/src/decode.rs`, `crates/mini_racer_wire/src/write.rs`, `crates/mini_racer_wire/src/text.rs` |
| The string standing in for a function | `crates/mini_racer_wire/src/marker.rs` |
| Wire codec benches without V8 or Ruby | `crates/mini_racer_wire/benches/wire.rs` |
| Golden wire bytes from V8, and V8 reading `Writer` | `crates/mini_racer_wire/tests/golden/`, `crates/mini_racer_engine/examples/gen_wire_golden.rs`, `crates/mini_racer_engine/tests/writer.rs` |
| Fuzz targets (wire codec, source maps) | `crates/mini_racer_wire/fuzz/fuzz_targets/`, `scripts/fuzz-smoke.sh` |
| Source map parsing, lookup, frame rewriting | `crates/mini_racer_sourcemap/src/map.rs`, `crates/mini_racer_sourcemap/src/vlq.rs`, `crates/mini_racer_sourcemap/src/rewrite.rs` |
| Registered source maps and matching evals | `crates/mini_racer_engine/src/process/source_maps.rs`, `crates/mini_racer_engine/src/runner.rs`, `ext/mini_racer_rs/src/source_maps.rs` |
| Per-context counters | `crates/mini_racer_engine/src/counters.rs`, `ext/mini_racer_rs/src/counters.rs` |
| Mutex poison policy | `crates/mini_racer_sync/src/lock.rs` |
| Turn-taking channel, parking, notifiers | `crates/mini_racer_sync/src/channel.rs`, `crates/mini_racer_sync/src/parker.rs` |
| Termination causes, stops, spent interrupts | `crates/mini_racer_sync/src/termination.rs` |
| Runner phases and the fork gate | `crates/mini_racer_sync/src/fork.rs` |
| std or loom primitives for protocol code | `crates/mini_racer_sync/src/primitives.rs` |
| Loom models | `crates/mini_racer_sync/tests/loom.rs` |
| `MiniRacer::Context` methods, per-context lock, large eval sources copied without the GVL | `ext/mini_racer_rs/src/context.rs`, `ext/mini_racer_rs/src/rb_raw.rs` |
| Waiting from Ruby threads, running callbacks | `ext/mini_racer_rs/src/host.rs` |
| Spin budgets and long-wait records | `ext/mini_racer_rs/src/spin.rs` |
| `Context.new` and `Pool.new` option rules | `ext/mini_racer_rs/src/options.rs` |
| `MiniRacerRs::Pool` | `ext/mini_racer_rs/src/pool.rs` |
| `MiniRacer::Snapshot` and its blob | `ext/mini_racer_rs/src/snapshot.rs` |
| `MiniRacer::Platform.set_flags!` | `ext/mini_racer_rs/src/platform.rs` |
| JavaScript to Ruby values | `ext/mini_racer_rs/src/decode.rs` |
| Ruby to JavaScript values | `ext/mini_racer_rs/src/encode.rs` |
| UTF-8 transcoded ahead of V8 | `crates/mini_racer_engine/src/text.rs`, `crates/mini_racer_engine/src/sys.rs` |
| Ruby values held by Rust storage | `ext/mini_racer_rs/src/gc_root.rs` |
| Conversions under `GC.stress` with compaction | `test/upstream/gc_stress.txt`, `test/ruby/conversion_gc_test.rb` |
| Error classes, C error messages, `ScriptError#cause` | `ext/mini_racer_rs/src/errors.rs` |
| GVL release, interrupts, raw rb-sys | `ext/mini_racer_rs/src/rb_raw.rs` |
| `Process._fork` and `Process.daemon` hooks | `ext/mini_racer_rs/src/fork.rs` |
| Fork cases, one process each | `test/ruby/fork_test.rb`, `test/ruby/fork/` |
| Conversions with little stack left | `ext/mini_racer_rs/src/decode.rs`, `ext/mini_racer_rs/src/host.rs`, `ext/mini_racer_rs/src/rb_raw.rs`, `ext/mini_racer_rs/src/errors.rs` |
| Loader and conflict guard | `lib/mini_racer_rs.rb` |
| Error hierarchy and Ruby compositions | `lib/mini_racer_rs/vocabulary.rb` |
| Native build, link flags | `ext/mini_racer_rs/extconf.rb`, `ext/mini_racer_rs/build.rs` |
| Export allowlists | `ext/mini_racer_rs/exports.allow.linux`, `ext/mini_racer_rs/exports.allow.macos` |
| V8 archive pins | `ext/mini_racer_rs/v8-archives.sha256`, `scripts/fetch-v8-archive.sh`, `scripts/update-v8-archives.sh` |
| Third-party notices for crates, std and V8 | `THIRD_PARTY_NOTICES.md`, `about.toml`, `about.hbs`, `licenses/v8/`, `licenses/rust/`, `scripts/update-notices.sh` |
| Dependency updates and advisory scans | `.github/dependabot.yml`, `.github/workflows/dependabot-auto-merge.yml`, `.github/workflows/osv-scan.yml`, `.github/workflows/ci.yml`, `osv-scanner.toml` |
| CI, and restoring the V8 static library | `.github/workflows/ci.yml`, `.github/actions/setup/action.yml`, `scripts/restore-v8-static-lib.sh` |
| musl builds and tests in Alpine | `scripts/alpine.sh`, `.github/workflows/ci.yml`, `.github/workflows/release.yml` |
| Alpine images pinned to index digests | `release/alpine/Dockerfile`, `scripts/alpine-image.sh`, `.github/dependabot.yml` |
| Releases: release PR, gem builds, publishing | `.github/workflows/release-please.yml`, `.github/workflows/release.yml`, `release-please-config.json`, `.release-please-manifest.json` |
| Gem packaging and the installed-gem smoke test | `rakelib/package.rake`, `release/smoke.rb`, `tools/versions.toml`, `.github/workflows/release.yml` |
| Release floors, gem sizes, publish order, lockfiles | `scripts/check-glibc-floor.sh`, `scripts/check-macos-minos.sh`, `scripts/check-gem-sizes.sh`, `scripts/publish-gems.sh`, `scripts/sync-release-locks.sh` |
| Sigstore attestations for published gems | `scripts/sign-gems.sh`, `release/sign/Gemfile`, `release/sign/Gemfile.lock`, `scripts/publish-gems.sh` |
| Sanitizer runs | `.github/workflows/nightly.yml`, `scripts/sanitizers.sh`, `.github/sanitizers/lsan.supp` |
| Nightly fuzzing and fresh-seed differential runs | `.github/workflows/nightly.yml`, `scripts/fuzz-smoke.sh`, `test/diff/run.rb` |
| Weekly Ruby head, source gem, newest v8 | `.github/workflows/extended.yml` |
| Weekly benchmark and its floors | `.github/workflows/bench.yml`, `bench/floors.rb` |
| Upstream suite runner, suites, guard, reporter | `test/upstream/run.rb`, `test/upstream/suites.rb`, `test/upstream/preload.rb`, `test/upstream/reporter.rb` |
| Per-test parity ledgers | `test/upstream/mini_racer.status.toml`, `test/upstream/execjs.status.toml`, `test/upstream/milestones.toml` |
| Ledger generation | `.github/workflows/ledger-generate.yml`, `rakelib/ledger.rake`, `test/upstream/milestone_rules.rb` |
| Upstream source (never edited) | `vendor/mini_racer` (v0.22.2), `vendor/execjs` (v2.10.2) |
| Differential run against the real gem | `test/diff/run.rb`, `test/diff/workers.rb`, `test/diff/worker.rb`, `test/diff/canonical.rb`, `test/diff/generate.rb`, `test/diff/shrink.rb`, `test/diff/ledger.rb`, `test/diff/shapes.rb` |
| Differential ledgers and corpus | `test/diff/candidate.status.toml`, `test/diff/divergences.toml`, `test/diff/corpus/` |
| Consumer workloads and their ledger | `test/consumers/workloads.rb`, `test/consumers/js/`, `test/consumers/candidate.status.toml` |
| rtlcss resolving through the shim | `test/consumers/transitive/` |
| Benchmarks against the real gem | `bench/run.rb`, `bench/measure.rb`, `bench/workloads.rb`, `bench/memory.rb`, `bench/external.rb`, `bench/fixtures/`, `bench/payload.rb`, `bench/serde.rb` |
| Memory, lifecycle, fork and spawn stress runs | `test/stress/`, `.github/workflows/nightly.yml` |
| Vendored TruffleRuby backend | `lib/mini_racer/truffleruby.rb`, `lib/mini_racer/shared.rb` |
| Public surface status and deviation reasons | `compat.toml`, `compat/surface.json` |
| Compat reflection, extraction, check, rendering | `tools/compat/`, `rakelib/compat.rake` |
| Generated from compat.toml (never edit) | `COMPATIBILITY.md`, the README compat table |
| README `ruby check` examples, each asserted | `test/ruby/readme_test.rb` |
| Public docs (outward-writing, Andy's approval) and templates | `README.md`, `ARCHITECTURE.md`, `BENCHMARKS.md`, `SECURITY.md`, `STABILITY.md`, `CONTRIBUTING.md`, `CODE_OF_CONDUCT.md` (Contributor Covenant 2.1, verbatim), `AI-DISCLOSURE.md` (copied verbatim), `.github/ISSUE_TEMPLATE/`, `.github/pull_request_template.md` |
| Weekly issue for a newer mini_racer | `.github/workflows/upstream-watch.yml`, `.github/upstream-release.md` |
| Issues for failed scheduled runs | `.github/workflows/scheduled-failures.yml`, `scripts/report-scheduled-failure.sh`, `scripts/check-scheduled-watch.sh` |

How the engine and the extension work past `ARCHITECTURE.md`, and why: `.claude/skills/engine-internals` (code cache, shared sources, fast calls, call arguments, primitive results, snapshots, source maps, heap limit, counters, forks and worker tasks, the watchdog) and `.claude/skills/extension-internals` (encoder strings, decoder keys, stack checks and scratch buffer, the pool, timeout scopes).

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
scripts/fuzz-smoke.sh 60           # every fuzz target, 60 s each, on the pinned nightly (a corpus directory may follow)
scripts/sanitizers.sh              # ASan teardown and memory checks, TSan on sync (Linux x86_64, needs llvm-symbolizer)
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
podman run --rm --init --userns=keep-id --user 0 --security-opt label=disable -v "$PWD:/mini_racer_rs" -w /mini_racer_rs "$(scripts/alpine-image.sh ruby-3.4)" scripts/alpine.sh test   # CI's Alpine job
```

Stress scripts and golden bytes: the testing skill. Benchmarks: the profiling skill. Packaging: the release-flow skill.

## Which tests to run

- `crates/mini_racer_wire`: `cargo nextest run -p mini_racer_wire`, then `scripts/fuzz-smoke.sh 20`; for a decode, write or transcode change, `cargo bench -p mini_racer_wire --bench wire` before and after. A new corpus entry goes in `crates/mini_racer_wire/tests/golden/corpus.tsv`; generate its `.bin`, then its `.txt`, and read the `.txt` before committing.
- `crates/mini_racer_sync`: `cargo nextest run -p mini_racer_sync`, then the loom command above.
- `crates/mini_racer_sourcemap`: `cargo nextest run -p mini_racer_sourcemap`, then `scripts/fuzz-smoke.sh 20`.
- `crates/mini_racer_engine`: `cargo nextest run -p mini_racer_engine`; for teardown or lifetime changes, `scripts/sanitizers.sh` or the `sanitizers` label. Flag-dependent assertions live in `crates/mini_racer_engine/tests/flags.rs`, one test per process.
- `ext/` or `lib/`: `cargo test -p mini_racer_rs --lib` (the Ruby-free option rules, number classification and spin record), `bundle exec rake test`, then `scripts/check-exports.sh`, then `bundle exec ruby test/upstream/run.rb --strict` with and without `--suite execjs`, then `bundle exec ruby test/diff/run.rb --strict` with and without `--set consumers`, then `bundle exec rake compat:check`.
- `compat.toml`: `bundle exec rake compat:render`, then `bundle exec rake compat:check`.
- `scripts/`, `layers.toml`, any `Cargo.toml`: `scripts/check-boundaries.sh`, `scripts/test-boundaries.sh` and `scripts/check-deny.sh`.
- `.github/`, `release/alpine/Dockerfile`: `scripts/check-action-pins.sh`, which also requires every container image a workflow runs to be pinned to a digest.
- `scripts/alpine.sh`, `scripts/fetch-v8-archive.sh`, `ext/mini_racer_rs/extconf.rb`: also CI's Alpine job under podman (the command above).
- `Cargo.lock`, `rust-toolchain.toml`, `licenses/`, `about.*`: `scripts/update-notices.sh --check`.

## Build traps

- A Rust edit reaches Ruby only after `bundle exec rake compile`: until then the stale `lib/mini_racer_rs/mini_racer_rs.so` keeps loading, and `NATIVE_VERSION` only catches version mismatches.
- An extension copied into `lib/mini_racer_rs/<minor>/` (from a native gem) wins over every build until it is deleted, and switching between a dev and a `CI=1` build can reinstall the other profile's copy; the debugging skill's "Stale extension" has the checks.
- Without the variables from `scripts/fetch-v8-archive.sh`, cargo downloads V8 itself and skips the pinned digest.
