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

`MiniRacer::Snapshot` builds and warms up a V8 startup blob in an isolate created for that purpose in `crates/mini_racer_engine/src/snapshot.rs`. Contexts made from one snapshot share one read-only `SnapshotBlob`, and `sys::startup_data` gives V8 those bytes without another copy. The context control block holds the blob until its runner drops the isolate because V8 reads it throughout the isolate's lifetime, including when creating contexts and checking code caches. A snapshot build holds the snapshot it warms up the same way. The snapshot creates its shared copy at `Context.new` and reuses it while a context made from it lives and the String returned by `dump` still contains the same bytes. It holds the copy weakly, so after those contexts are gone it holds only that String. Each context starts from the String's contents when `Context.new` runs.

`SnapshotBlob::check` follows V8 15.2's `SnapshotImpl` blob layout in `src/snapshot/snapshot.cc`. It checks the header, the version through V8's `StartupData::is_valid`, the context count and rehash flag, and that every section begins with the same magic number, whose top half is V8's `0xC0DE`, and fits its recorded payload length. It accepts an empty blob, bytes after the last section, and damaged payload bytes that V8 accepts because it reads no checksum and leaves payload contents to V8. It refuses damage in a section's 8-byte header. See compatibility deviation `behavior:invalid-snapshot@ruby`.

## Code cache

Contexts in one process share a V8 code cache for evaluated sources of 1 MiB or more (for a source held as UTF-16, half that many code units). The first sighting digests source samples. At the second sighting with the same file name and startup snapshot, the runner digests the whole source with a key chosen at random per process and, if the eval succeeds, builds the cache once the dispatch is over and before the eval returns. The keyed digest protects against V8's length-only cache check. Building is outside `timeout:` but delays return. A `Context#stop` or Ruby `Timeout` arriving during the build applies to the next script. A fork immediately after return finds the context idle.

Later evals compile from cached code. Simultaneous loaders build one entry while others compile from source. V8 rejects entries from other flags, versions, or startup snapshots, so the runner drops them and compiles from source. The cache holds at most eight entries and 256 MiB, evicts least recently used entries first, and neither keeps nor rebuilds an entry larger than 256 MiB. An entry stays found however many other large scripts are seen after it, and forked children inherit entries. On one machine, a 44 MB production rendering bundle loaded in about 200 ms from cache, versus about 1,000 ms from source and with mini_racer 0.22.1. The implementation is in `crates/mini_racer_engine/src/process/code_cache.rs` and `crates/mini_racer_engine/src/runner.rs`.


## Fast calls

When `Context#eval` receives a source without a file name in one of two supported shapes, mini_racer_rs runs a template compiled once by that context instead of compiling the source. The shapes are ExecJS 2.10.2's `call`, `(<path>.apply(this, <JSON array>))`, and the `compose_js` wrapper used by react-rails 3.2.1 and 3.3.1 around one `this.ReactRailsUJS.serverRender('<function>', '<component>', <props>)` call. `crates/mini_racer_engine/src/fast_calls.rs` recognizes these shapes, and `crates/mini_racer_engine/src/runner.rs` runs them.

The template is a top-level script like the original source. It replaces each JSON literal with a call to a native function held in a randomly named global `const`, and V8's JSON parser reads the values. Results, error classes and messages, `this`, and stack frame line and column numbers remain those of the source. Recognition falls back to the normal path for another shape, non-JSON values, `__proto__` in any spelling, nesting past 128, a line break in render props, a template over 1 MiB, `verbose_exceptions:`, or a repeated source equal to the context's last fast call. It also rejects render code around the call that cannot compile independently or uses a backtick, `eval`, `caller`, a `\u` escape, or `arguments` except as a property after `.` on the same line.

`MINI_RACER_RS_FAST_CALLS=off`, set before the first context is created, disables fast calls process-wide; forked children retain the setting. Reflection-only differences are recorded under `behavior:fast-calls@ruby`.

## Forks

With the `:single_threaded` platform flag, a runner serves its isolate as a `v8::SharedIsolate`. Between requests, the isolate remains unlocked in its control block.

Hooks on `Process._fork` and `Process.daemon` in `ext/mini_racer_rs/src/fork.rs` cover Ruby `fork`, `IO.popen("-")`, and `Process.daemon`. Their `pthread_atfork` prepare handler waits up to 500 ms for every runner to become idle or parked, interrupting JavaScript after 50 ms. In every process, mini_racer_rs points V8's platform at a copied vtable whose worker-thread slots post to a pool it owns. V8's platform has one worker thread, which never receives a task. The pool has as many threads as V8 would run for the mode: the processors online, held to 1 through 16, or 1 under `:single_threaded`. Its threads start at the first worker task. In both modes, the prepare handler pauses the pool so no task starts, then waits up to 100 ms for running tasks to end. These waits are best effort. Past either bound, the fork proceeds, the child refuses every inherited context, and its new contexts are best effort. The parent resumes the pool when the fork returns. Each child starts its own pool and never touches its parent's; tasks queued in the parent's pool are not run in the child. Idle pool threads wait on a Rust standard library condition variable, which keeps no records on their stacks for a child to inherit. The pool lives in `crates/mini_racer_engine/src/process/workers.rs` and `crates/mini_racer_sync/src/workers.rs`; `crates/mini_racer_engine/src/sys.rs` copies the vtable and runs and deletes V8's tasks. The fork protocol is in `crates/mini_racer_engine/src/process/fork.rs` and `crates/mini_racer_sync/src/fork.rs`.

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
* V8-owned objects are dropped on a thread of the current fork generation: a runner, or for a worker task, a thread of that generation's worker pool.

## How it is checked

The unmodified mini_racer and ExecJS test suites run from submodules against per-platform ledgers in `test/upstream/`. Differential tests in `test/diff/` send the same generated values and the Babel, Terser, Handlebars, and React libraries to mini_racer 0.22.1 and this gem.

Loom models exercise the channel, termination, and fork protocols. cargo-fuzz targets cover the wire codec. AddressSanitizer, ThreadSanitizer, fork and spawn stress, and garbage collection stress with compaction run in CI or nightly.

Every difference from mini_racer 0.22.1 is recorded in `compat.toml` and listed in `COMPATIBILITY.md`.
