# Architecture

`mini_racer_rs` implements the `MiniRacer` Ruby API from mini_racer 0.22.1 in Rust, using V8 15.2 through `v8` (rusty_v8 152.2.0) and Ruby bindings through magnus and rb-sys. Rust owns behavior, while `lib/` defines vocabulary: constants, the error hierarchy, `MiniRacer::Binary`, the loader, `Context#load`, `Context#write_heap_snapshot`, and mini_racer's unchanged TruffleRuby backend in `lib/mini_racer/truffleruby.rb` and `lib/mini_racer/shared.rb`.

## Crates

All four crates have `publish = false`. `scripts/check-boundaries.sh` enforces their layering against `layers.toml`.

| Crate | Layer | Role |
|---|---:|---|
| `mini_racer_wire` | 0 | V8 wire format codec. It uses `forbid(unsafe_code)`. |
| `mini_racer_sync` | 0 | Concurrency protocol types, checked with loom. It uses `forbid(unsafe_code)`. |
| `mini_racer_engine` | 1 | V8 process state, flags, and runners. This is the only crate that names `v8`. |
| `mini_racer_rs` | 2 | Ruby extension built as a `cdylib`. This is the only crate that names magnus or rb-sys. |

## A call, end to end

Each `MiniRacer::Context` owns a runner thread implemented in `crates/mini_racer_engine/src/runner.rs`. Its stack is at least 2 MiB, so V8 frames never run on a Ruby thread's stack.

One dispatch at a time enters a context through a per-context Ruby `Mutex`. The extension encodes the request into a byte buffer and sends it over the channel in `crates/mini_racer_sync/src/channel.rs`. The Ruby thread and runner then take turns: the runner receives the request, operates on V8, and returns either a response or a callback request.

While waiting, the Ruby side first spins for 20 microseconds with the GVL held, then for up to 50 more without it, and finally parks with the GVL released. This logic lives in `ext/mini_racer_rs/src/host.rs`. `MINI_RACER_RS_SPIN_US` is an unstable knob for these budgets.

A signal trap wakes the park and waiting continues. If a Ruby interrupt raises an exception, the extension cancels the dispatch, terminates it, waits for the runner to answer, and then raises the exception.

The context handle in `crates/mini_racer_engine/src/context.rs` coordinates dispatch, nested dispatch from callbacks, the `Host` contract implemented by the extension, `stop`, and `dispose`.

## Values

Values cross the channel in V8's serialization format. Results are decoded from format versions 13 through 16 by `mini_racer_wire`, using a visitor that constructs Ruby objects directly in `ext/mini_racer_rs/src/decode.rs`. There is no intermediate tree, nesting is limited to 96 levels, and repeated objects preserve their identity.

Arguments to `Context#call` travel in the other direction. `ext/mini_racer_rs/src/encode.rs` writes format version 15 directly from Ruby values.

Before serializing a result, the runner sanitizes it as mini_racer does. If V8 cannot clone the value, `crates/mini_racer_engine/src/serialize.rs` retries through mini_racer's filter in a separate safe context.

## Termination and limits

Each control block contains a termination cell from `crates/mini_racer_sync/src/termination.rs`. It records stop, timeout, cancel, and memory causes together with the dispatch to which they belong.

Dispatch identity preserves two important behaviors. A stop requested while the context is idle is reported by the next script, and a late timeout cannot terminate the following dispatch.

`Context.new(timeout:)` is enforced by one process-wide watchdog thread in `crates/mini_racer_engine/src/process/watchdog.rs`.

## Callbacks

`Context#attach` installs a JavaScript function backed by a trampoline. When JavaScript calls it, the runner sends a callback request over the channel, the Ruby thread executes the attached callable, and the reply returns over the channel.

A call into the same context from inside that callback runs as a nested dispatch within the dispatch that invoked the callback.

## Promises and V8's tasks

`Context#eval_await` and `Context#call_await` await a promise result through `await_promise` in `crates/mini_racer_engine/src/runner.rs`. The runner performs a microtask checkpoint and inspects the promise. While it remains pending, the runner executes due V8 tasks one at a time and sleeps until the next task is due. This continues until the promise settles or the dispatch is stopped, timed out, cancelled, or disposed. An await nested in a callback is refused with `nested call_await/eval_await is not supported`, because V8 runs no microtask checkpoint inside another.

`Context#pump_message_loop` executes one due task, performs a microtask checkpoint if a task ran, and returns whether one ran. `Context#perform_microtask_checkpoint` executes the queued microtasks. The engine initializes V8 with `v8::new_custom_platform` in `crates/mini_racer_engine/src/process.rs`, which routes foreground tasks to Rust. Each isolate has a `TaskQueue` in `crates/mini_racer_sync/src/tasks.rs`. It preserves posting order, makes delayed tasks available when due, and runs non-nestable tasks only outside another task.

A registry per forked process maps isolate addresses to queues in `crates/mini_racer_engine/src/process/tasks.rs`. The runner registers the queue after creating the isolate and removes it after the isolate is gone. Tasks posted during teardown are therefore dropped instead of reaching a later isolate at the same address. Tasks for isolates without a serving runner, including isolates used to build snapshots, are dropped when posted.

Termination rings the queue, waking a runner sleeping in an await without posting a wakeup task. Such a runner holds its isolate but runs nothing in it, so a fork does not wait for it. An idle context taken over by a forked child keeps its queued tasks. The loom models in `crates/mini_racer_sync/tests/loom.rs` cover wakeup by stop, posting while a runner decides to sleep, and posting while a queue closes.

## Snapshots

`MiniRacer::Snapshot` builds a V8 startup blob in an isolate created for that purpose. The implementation is in `crates/mini_racer_engine/src/snapshot.rs`, and a context can start from the resulting blob.

## Forks

With the `:single_threaded` platform flag, a runner serves its isolate as a `v8::SharedIsolate`. Between requests, the isolate remains unlocked in its control block.

Hooks on `Process._fork` and `Process.daemon` in `ext/mini_racer_rs/src/fork.rs` cover Ruby `fork`, `IO.popen("-")`, and `Process.daemon`. Their `pthread_atfork` prepare handler waits up to 500 ms for every runner to become idle or parked, interrupting JavaScript after 50 ms. In both modes, and only in the process that started V8, it then waits up to 100 ms for every worker to run a posted holding task. The `:single_threaded` platform has one worker, which V8's scavenger still posts jobs to. V8 wakes one idle worker per post, and its list can retain workers whose timed waits expired, so another task is posted every millisecond while workers remain free. Each task holds its worker until the fork returns in the parent. These waits are best effort, and the fork proceeds when either bound expires. A held worker is neither queued on V8's condition variable nor holding its lock, so the child's posts find no waiter only when every worker was held. A forked child has no V8 workers and posts nothing. The protocol is in `crates/mini_racer_engine/src/process/fork.rs` and `crates/mini_racer_sync/src/fork.rs`; `crates/mini_racer_engine/src/sys.rs` posts tasks to V8 workers.

In the child, each context that was idle receives a new runner around the same isolate. A context that was in use at the fork raises in the child. The child handler only increments a fork epoch, and processes are distinguished by that epoch rather than by pid. The child never frees or waits on inherited objects.

V8 marks its memory with `MADV_DONTFORK`. The `madvise` wrapper linked into the extension drops that flag, allowing the child to use the isolate it takes over. This wrapper lives in `crates/mini_racer_engine/src/sys.rs`.

## Fiber schedulers

Under a fiber scheduler, a fiber waiting for its context parks through the scheduler's `io_wait` on a per-context notification descriptor. `crates/mini_racer_engine/src/notify.rs` provides the descriptor, allowing other fibers on the thread to continue running.

## Rules that keep it safe

* `unsafe` appears only in the modules fenced by `layers.toml`: `engine::sys`, `ext::rb_raw`, `ext::guard`, and `ext::gc_root`. Each block contains one operation and a `// SAFETY:` comment.
* All rb-sys calls live in `ext/mini_racer_rs/src/rb_raw.rs`.
* A Ruby exception or throw never unwinds through a Rust frame. Ruby calls use `rb_protect`, and a pending jump resumes only after Rust frames return.
* Every entry point from Ruby or V8 into Rust is guarded against panics.
* Ruby values stored by Rust pass through `ext/mini_racer_rs/src/gc_root.rs`, allowing the garbage collector to mark them.
* Isolate-scoped V8 calls run only on the thread that owns or locks the isolate. Other threads access it only to terminate, cancel, or interrupt it.
* V8-owned objects are dropped on a runner thread belonging to the current fork generation.

## How it is checked

The unmodified mini_racer and ExecJS test suites run from submodules against per-platform ledgers in `test/upstream/`. Differential tests in `test/diff/` send the same generated values and the Babel, Terser, Handlebars, and React libraries to mini_racer 0.22.1 and this gem.

Loom models exercise the channel, termination, and fork protocols. cargo-fuzz targets cover the wire codec. AddressSanitizer, ThreadSanitizer, fork and spawn stress, and garbage collection stress with compaction run in CI or nightly.

Every difference from mini_racer 0.22.1 is recorded in `compat.toml` and listed in `COMPATIBILITY.md`.
