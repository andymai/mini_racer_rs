# Architecture

`mini_racer_rs` implements the `MiniRacer` Ruby API from mini_racer 0.22.2 in Rust, using V8 15.2 through `v8` (rusty_v8 152.2.0) and Ruby bindings through magnus and rb-sys. Rust owns behavior, while `lib/` defines vocabulary: constants, the error hierarchy, `MiniRacer::Binary`, the loader, `Context#load`, `Context#write_heap_snapshot`, and mini_racer's unchanged TruffleRuby backend in `lib/mini_racer/truffleruby.rb` and `lib/mini_racer/shared.rb`.

## Crates

All five crates have `publish = false`. `scripts/check-boundaries.sh` enforces their layering against `layers.toml`.

| Crate | Layer | Role |
|---|---:|---|
| `mini_racer_wire` | 0 | V8 wire format codec. It uses `forbid(unsafe_code)`. |
| `mini_racer_sync` | 0 | Concurrency protocol types, checked with loom. It uses `forbid(unsafe_code)`. |
| `mini_racer_sourcemap` | 0 | Source maps for JavaScript backtraces: parsing, lookup and rewriting frame locations. It uses `forbid(unsafe_code)`. |
| `mini_racer_engine` | 1 | V8 process state, flags, and runners. This is the only crate that names `v8`. |
| `mini_racer_rs` | 2 | Ruby extension built as a `cdylib`. This is the only crate that names magnus or rb-sys. |

## A call, end to end

Each `MiniRacer::Context` owns a runner thread implemented in `crates/mini_racer_engine/src/runner.rs`. Its stack is at least 2 MiB, so V8 frames never run on a Ruby thread's stack.

One dispatch at a time enters a context through a per-context Ruby `Mutex`. The extension encodes the request into a byte buffer and sends it over the channel in `crates/mini_racer_sync/src/channel.rs`. The Ruby thread and runner then take turns: the runner receives the request, operates on V8, and returns either a response or a callback request.

While waiting, the Ruby side first spins for 20 microseconds with the GVL held, then for up to 50 more without it, and finally parks with the GVL released. Each context counts consecutive waits whose answers arrive after the spin budget. After 3 in a row, it skips spinning and parks at once, releasing the GVL right away, because longer answers make spinning burn the caller's CPU and initially hold the GVL before parking anyway. A skipped wait times itself, and spinning resumes when an answer arrives within the budget. The budgets and rule live in `ext/mini_racer_rs/src/spin.rs`; waiting stays in `ext/mini_racer_rs/src/host.rs`. `MINI_RACER_RS_SPIN_US` is an unstable knob for these budgets.

A signal trap wakes the park and waiting continues. If a Ruby interrupt raises an exception, the extension cancels the dispatch, terminates it, waits for the runner to answer, and then raises the exception.

The context handle in `crates/mini_racer_engine/src/context.rs` coordinates dispatch, nested dispatch from callbacks, the `Host` contract implemented by the extension, `stop`, and `dispose`.

## Values

Values cross the channel in V8's serialization format. Results are decoded from format versions 13 through 16 by `mini_racer_wire`, using a visitor that constructs Ruby objects directly in `ext/mini_racer_rs/src/decode.rs`. There is no intermediate tree, repeated objects preserve their identity, results may be nested 95 levels, and callback arguments may be nested 96 levels counting the arguments array, matching mini_racer 0.22.2.

Arguments to `Context#call` travel in the other direction. `ext/mini_racer_rs/src/encode.rs` writes format version 15 directly from Ruby values. It refuses an argument nested deeper than 512 levels with `Context.call: max depth exceeded`, as mini_racer 0.22.2 does.

The encoder in `ext/mini_racer_rs/src/encode.rs` writes Ruby Strings sent as UTF-8 as V8 one-byte text when Ruby's cached coderange, or a byte scan when no coderange exists, identifies all-ASCII input. For valid non-ASCII UTF-8 from 16 bytes through 8 MiB, bundled simdutf decoding produces V8's one-byte or padded two-byte storage form, while broken UTF-8, larger values, Hash keys, and call names retain mini_racer's form, and the runner builds calls holding padded or wide two-byte strings itself.

Primitive results, including small integers, heap numbers other than NaN, strings, booleans, null, and undefined, are written byte for byte by `serialize::primitive` in `crates/mini_racer_engine/src/serialize.rs` as V8's serializer writes them; other results go through V8's serializer. The encoder writes `[name, *args]` directly from method arguments without building a Ruby array. After the byte-only `request::buildable` pass in `crates/mini_racer_engine/src/request.rs` approves the whole call, the runner builds packed arguments with at most 4 pairs total or at most 2 per hash on average from those bytes; V8's deserializer reads every other call whole.

Before serializing a result, the runner sanitizes it as mini_racer does. If V8 cannot clone the value, `crates/mini_racer_engine/src/serialize.rs` retries through mini_racer's filter in a separate safe context.

The result decoder interns object and Map string keys and uses the per-decode cache Ruby's json gem uses for objects inside arrays, up to 63 keys of at most 55 bytes. Each Hash takes its pending pairs through one bulk insert when it ends; the pairs stay on the native stack or, past 128 values, in a garbage collector marked holder, and every open hash flushes before a container Map key can expose it. Latin-1 and UTF-16 strings transcode in one pass through a reused buffer and copy once into Ruby Strings; text whose UTF-8 could pass 64 KiB is measured first and receives an exact-size buffer.

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

`MiniRacer::Snapshot` builds and warms up a V8 startup blob in a purpose-built isolate in `crates/mini_racer_engine/src/snapshot.rs`. Contexts made from one snapshot share one read-only `SnapshotBlob`, and `sys::startup_data` gives V8 those bytes directly. A context's control block retains the blob until its runner drops the isolate because V8 reads it throughout the isolate's lifetime, including when creating contexts and checking code caches. Snapshot builds retain the snapshot they warm up the same way. The shared `SnapshotBlob` is made at `Context.new` and reused while a context made from it lives. The snapshot holds it weakly, so once its contexts are gone, it holds only its own String. Like mini_racer 0.22.2, `dump` returns a copy of that String, so changing the copy does not change what a context starts from.

`Snapshot.new` runs mini_racer 0.22.2's `Intl` guard script word for word before its script. A script that reads or writes `Intl` raises `MiniRacer::SnapshotError` with `Intl is not supported in Snapshot.new scripts`, because V8 cannot write `Intl` objects into a blob and aborts the process on one. The guard restores `Intl` after a script that did not access it. `#warmup!` has no guard. Warming up an empty snapshot warms up V8's own snapshot, as mini_racer 0.22.2 does.

`SnapshotBlob::check` makes mini_racer 0.22.2's checks, in its order and with its messages, on V8 15.2's blob layout (`SnapshotImpl` in V8's `snapshot.cc`): the length, the version through V8's `StartupData::is_valid`, the header, each section's offset, then the checksum V8 writes in word 8 over everything after it. It accepts Adler-32, written by libv8-node's V8 built with zlib, or V8's own Fletcher-32, written by the `v8` crate's V8. It then rejects two blobs mini_racer hands to V8: a section without V8's magic number, which V8 aborts on, and a recorded payload that runs past its section, which V8 reads unchecked. The magic word is pinned as `SnapshotBlob::MAGIC` and compared whole, as V8 compares it. A matching checksum prevents neither case because a blob writer can also write its checksum. An empty blob creates a context without a snapshot. Damaged payload or trailing bytes under a matching checksum load when V8 can read them, as in mini_racer 0.22.2. See compatibility deviation `behavior:invalid-snapshot@ruby`.

## Code cache

Contexts in one process share a V8 code cache for evaluated sources of 128 KiB or more, or half that many code units when a source is held as UTF-16. At the first sighting of a source of 1 MiB or more, a context that shares sources, meaning any context created without `max_memory:`, digests the whole source with a key chosen at random per process so it can read the source from a shared copy. A `max_memory:` context, and any context evaluating a smaller source, digests source samples. At the second sighting with the same file name and startup snapshot, the runner digests the whole source. If evaluation succeeds, it builds the cache after dispatch finishes and before evaluation returns. A shared source of 16 MiB or more is cached at its first sighting instead when its top-level code ran at least as long as it took to compile. The context that built an entry rebuilds it once after its next `Context#call` or eval under 128 KiB, so the entry also holds the functions that call compiled; a collection the runner requests drops a pending rebuild. The keyed digest protects against V8's length-only cache check. Cache building and rebuilding are outside `timeout:` but delay return. A `Context#stop` or Ruby `Timeout` received during a build applies to the next script. A fork immediately after return finds the context idle.

Later evals compile from cached code. Simultaneous loaders build one entry while others compile from source. V8 rejects entries from other flags, versions, or startup snapshots, so the runner drops them and compiles from source. The cache holds at most 64 entries and 256 MiB, evicts least recently used entries first, keeps a first-sighting entry only if no entry a context has read must go to make room, and neither keeps nor rebuilds an entry larger than 256 MiB. An entry stays found however many other large scripts are seen after it, and forked children inherit entries. On one machine, a 44 MB production rendering bundle loaded in about 200 ms from cache, versus about 1,000 ms from source and with mini_racer 0.22.1. The implementation is in `crates/mini_racer_engine/src/process/code_cache.rs` and `crates/mini_racer_engine/src/runner.rs`.


## Shared sources

From its first evaluation, a context evaluating a source of 1 MiB or more under the code cache key, consisting of the file name, startup snapshot, length, and keyed digest, gives V8 an external string over one shared, immutable copy in `crates/mini_racer_engine/src/process/sources.rs`. Every later context evaluating the same source reads that copy. The copy uses one byte per character when every character fits Latin-1 and two otherwise, matching V8's storage. Each V8 string holds one reference, which V8 releases once when it collects the string, internalizes it, or tears down its isolate. Release callbacks in `crates/mini_racer_engine/src/sys.rs` only decrement a reference count.

The process keeps at most 8 copies and 256 MiB. At the next large evaluation of a shared source, it drops a copy read by only one context once no string reads it, so a script loaded once does not stay resident. If that source is among the last 8 dropped, its new copy counts as shared. Copies read by two or more contexts, and all copies inherited by forked children, remain until a new source needs room. Eviction proceeds least recently used first. Room is decided before conversion, so a full list costs no conversion. Sources past the bounds, evaluations under `max_memory:`, invalid UTF-8 sources, and evaluations with `MINI_RACER_RS_CODE_CACHE=off` use context-local copies. A forked child copies the list and never frees an inherited copy. `behavior:shared-source-heap-stats@ruby` records the approved heap statistics deviation.

## Fast calls

When `Context#eval` receives a source without a file name in one of two supported shapes, mini_racer_rs runs a template compiled once by that context instead of compiling the source. The shapes are ExecJS 2.10.2's `call`, `(<path>.apply(this, <JSON array>))`, and the `compose_js` wrapper used by react-rails 3.2.1 and 3.3.1 around one `this.ReactRailsUJS.serverRender('<function>', '<component>', <props>)` call. `crates/mini_racer_engine/src/fast_calls.rs` recognizes these shapes, and `crates/mini_racer_engine/src/runner.rs` runs them.

The template is a top-level script like the original source. It replaces each JSON literal with a call to a native function held in a randomly named global `const`, and V8's JSON parser reads the values. Results, error classes and messages, `this`, and stack frame line and column numbers remain those of the source. Recognition falls back to the normal path for another shape, non-JSON values, `__proto__` in any spelling, nesting past 128, a line break in render props, a template over 1 MiB, `verbose_exceptions:`, or a repeated source equal to the context's last fast call. It also rejects render code around the call that cannot compile independently or uses a backtick, `eval`, `caller`, a `\u` escape, or `arguments` except as a property after `.` on the same line.

`MINI_RACER_RS_FAST_CALLS=off`, set before the first context is created, disables fast calls process-wide; forked children retain the setting. Reflection-only differences are recorded under `behavior:fast-calls@ruby`.

## Source maps

On CRuby, `MiniRacerRs.register_source_map(bundle, map)` parses a version 3 source map once, without the GVL, into a process-wide registry of at most eight bundles found by content. Parsing, lookup, and frame rewriting live in the `mini_racer_sourcemap` crate, which has no unsafe code.

Before a script runs, `runner::match_bundle` matches a UTF-8 source containing a registered bundle. The isolate then captures detailed stack traces, and `runner::relocated` rewrites matching `MiniRacer::RuntimeError` frames to original files, lines, and columns, keeping V8's positions for frames it cannot pair. A forked child starts from a copy of the registry.

## Forks

With the `:single_threaded` platform flag, a runner serves its isolate as a `v8::SharedIsolate`. Between requests, the isolate remains unlocked in its control block.

Hooks on `Process._fork` and `Process.daemon` in `ext/mini_racer_rs/src/fork.rs` cover Ruby `fork`, `IO.popen("-")`, and `Process.daemon`. Their `pthread_atfork` prepare handler waits up to 500 ms for every runner to become idle or parked, interrupting JavaScript after 50 ms. In every process, mini_racer_rs points V8's platform at a copied vtable whose worker-thread slots post to a pool it owns. V8's platform has one worker thread, which never receives a task. The pool has as many threads as V8 would run for the mode: the processors online, held to 1 through 16, or 1 under `:single_threaded`. Its threads start at the first worker task. In both modes, the prepare handler pauses the pool so no task starts, then waits up to 100 ms for running tasks to end. These waits are best effort. Past either bound, the fork proceeds, the child refuses every inherited context, and its new contexts are best effort. The parent resumes the pool when the fork returns. Each child starts its own pool and never touches its parent's; tasks queued in the parent's pool are not run in the child. Idle pool threads wait on a Rust standard library condition variable, which keeps no records on their stacks for a child to inherit. The pool lives in `crates/mini_racer_engine/src/process/workers.rs` and `crates/mini_racer_sync/src/workers.rs`; `crates/mini_racer_engine/src/sys.rs` copies the vtable and runs and deletes V8's tasks. The fork protocol is in `crates/mini_racer_engine/src/process/fork.rs` and `crates/mini_racer_sync/src/fork.rs`.

Under `:single_threaded`, each context that was idle at the fork receives a new runner around the same isolate in the child. A context that was in use at the fork raises in the child. In the default mode, the child refuses every inherited context with `MiniRacer::RuntimeError`. The child handler only increments a fork epoch, and processes are distinguished by that epoch rather than by pid. The child never frees or waits on inherited objects.

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

The unmodified mini_racer and ExecJS test suites run from submodules against per-platform ledgers in `test/upstream/`. Differential tests in `test/diff/` send the same generated values and the Babel, Terser, Handlebars, React, and MUI libraries to mini_racer 0.22.2 and this gem.

Loom models exercise the channel, termination, fork, per-isolate task queue, and worker task queue pause and resume protocols. cargo-fuzz targets cover the wire codec and source map parser. AddressSanitizer, ThreadSanitizer, fork and spawn stress, and garbage collection stress with compaction run in CI or nightly.

Every difference from mini_racer 0.22.2 is recorded in `compat.toml` and listed in `COMPATIBILITY.md`.
