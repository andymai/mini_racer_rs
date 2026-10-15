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
| V8 initialization and version | `crates/mini_racer_engine/src/process.rs` |
| Runner thread, script evaluation, serialization | `crates/mini_racer_engine/src/runner.rs` |
| Context handle, waiting, termination, fork guard | `crates/mini_racer_engine/src/context.rs` |
| Engine errors | `crates/mini_racer_engine/src/error.rs` |
| `madvise` wrapper (fork safety) | `crates/mini_racer_engine/src/sys.rs`, `crates/mini_racer_engine/build.rs` |
| Wire decoding and text transcoding | `crates/mini_racer_wire/src/decode.rs`, `crates/mini_racer_wire/src/text.rs` |
| `MiniRacer::Context` methods | `ext/mini_racer_rs/src/context.rs` |
| `MiniRacer::Platform.set_flags!` | `ext/mini_racer_rs/src/platform.rs` |
| JavaScript to Ruby values | `ext/mini_racer_rs/src/decode.rs` |
| Error class mapping | `ext/mini_racer_rs/src/errors.rs` |
| GVL release, interrupts, raw rb-sys | `ext/mini_racer_rs/src/rb_raw.rs` |
| Loader and conflict guard | `lib/mini_racer_rs.rb` |
| Error hierarchy and Ruby compositions | `lib/mini_racer_rs/vocabulary.rb` |
| Native build, link flags | `ext/mini_racer_rs/extconf.rb`, `ext/mini_racer_rs/build.rs` |
| Export allowlists | `ext/mini_racer_rs/exports.allow.linux`, `ext/mini_racer_rs/exports.allow.macos` |
| V8 archive pins | `ext/mini_racer_rs/v8-archives.sha256`, `scripts/fetch-v8-archive.sh` |
| CI | `.github/workflows/ci.yml`, `.github/actions/setup/action.yml` |
| Upstream suite runner, guard, reporter | `test/upstream/run.rb`, `test/upstream/preload.rb`, `test/upstream/reporter.rb` |
| Per-test parity ledger | `test/upstream/mini_racer.status.toml`, `test/upstream/milestones.toml` |
| Ledger generation | `.github/workflows/ledger-generate.yml`, `rakelib/ledger.rake`, `test/upstream/milestone_rules.rb` |
| Upstream source (never edited) | `vendor/mini_racer` (v0.22.1), `vendor/execjs` (v2.10.2) |
| Vendored TruffleRuby backend | `lib/mini_racer/truffleruby.rb`, `lib/mini_racer/shared.rb` |

Skills: `.claude/skills/pr-workflow` (branch to merge), `.claude/skills/layer-boundaries` (dependencies, fences, check failures), `.claude/skills/parity-suite` (upstream suite and ledger).

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
bundle exec ruby test/upstream/run.rb --strict   # mini_racer's own suite against the ledger
```

## Which tests to run

- `crates/mini_racer_wire`: `cargo nextest run -p mini_racer_wire`.
- `crates/mini_racer_engine`: `cargo nextest run -p mini_racer_engine`. Flag-dependent assertions live in `crates/mini_racer_engine/tests/flags.rs`, one test per process.
- `ext/` or `lib/`: `bundle exec rake test`, then `scripts/check-exports.sh`, then `bundle exec ruby test/upstream/run.rb --strict`.
- `scripts/`, `layers.toml`, any `Cargo.toml`: `scripts/check-boundaries.sh` and `scripts/test-boundaries.sh`.
- `.github/`: `scripts/check-action-pins.sh`.

## Pitfalls

- `lib/mini_racer_rs/version.rb` holds one version literal; release-please rewrites the first one it finds.
- `rake compile` builds the dev profile locally and release when `CI` is set (`RB_SYS_CARGO_PROFILE`).
- A stale `lib/mini_racer_rs/mini_racer_rs.so` keeps loading after Rust edits until `rake compile` runs; `NATIVE_VERSION` only catches version mismatches.
- Without the variables from `scripts/fetch-v8-archive.sh`, cargo downloads V8 itself and skips the pinned digest.
- V8's platform and flags are process-global and freeze at first use, so tests that set flags run in their own process.
- `mini_racer_rs.gemspec` lists files by explicit globs; a new file outside them is missing from the gem.
- Source builds need libclang; with the `libclang` gem, `ext/mini_racer_rs/extconf.rb` also passes GCC's include directory to bindgen.
- Each git worktree needs its own `npm ci` before the commit hooks can run, and `git submodule update --init` before the upstream suite can run.
- The upstream suite runs from `vendor/mini_racer` with `BUNDLE_GEMFILE`, `RUBYOPT` and `MINI_RACER_RS_UPSTREAM_REPORT` set by `test/upstream/run.rb`; run it through that script, not with `rake test` inside the submodule.
- In a forked child, contexts inherited from the parent raise `MiniRacer::RuntimeError` (`single-threaded runner: ...`).
