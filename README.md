<div align="center">

# mini_racer_rs

A drop-in replacement for mini_racer, implemented in Rust on V8 15.2.

[![CI](https://github.com/andymai/mini_racer_rs/actions/workflows/ci.yml/badge.svg)](https://github.com/andymai/mini_racer_rs/actions/workflows/ci.yml)
[![Commit activity](https://img.shields.io/github/commit-activity/m/andymai/mini_racer_rs?label=commits%2Fmonth)](https://github.com/andymai/mini_racer_rs/commits/main)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Ruby 3.3 | 3.4 | 4.0](https://img.shields.io/badge/ruby-3.3%20%7C%203.4%20%7C%204.0-red.svg)](#scope)
[![V8 15.2](https://img.shields.io/badge/V8-15.2-4285F4.svg)](#how-it-works)
[![mini_racer 0.22.1 suite](https://img.shields.io/badge/mini__racer%200.22.1%20suite-passing-success.svg)](COMPATIBILITY.md)

**[Install](#install)** · **[Quick start](#quick-start)** · **[API](#api-at-a-glance)** · **[Switching](#switching-from-mini_racer)** · **[Benchmarks](#benchmarks)** · **[How it works](#how-it-works)** · **[Compatibility](COMPATIBILITY.md)** · **[Architecture](ARCHITECTURE.md)**

</div>

mini_racer_rs implements the `MiniRacer` API from mini_racer 0.22.1. It defines `MiniRacer` and ships `lib/mini_racer.rb`, so `require "mini_racer"` and ExecJS keep working.

> mini_racer_rs is an independent project and is not affiliated with the Discourse staff who maintain mini_racer.

## Highlights

- **Round trips:** 5 to 9 times faster (0.72 µs vs 6.51 µs per call).
- **Babel through ExecJS:** 1.95x faster (5.15 ms vs 10.05 ms).
- **Bundle load:** 4.6x faster for a 44 MB production server rendering bundle (215 ms vs 983 ms).
- **Memory:** A context holding the MUI bundle uses 44 MB vs 110 MB.
- **Forks:** After loading and rendering React: 20 of 20 clean mini_racer_rs exits vs 20 of 20 mini_racer aborts.
- **V8:** V8 15.2.124.1 vs V8 13.6.233.17 in mini_racer 0.22.1.
- **No compiler:** Native gems cover glibc and musl Linux, including Alpine, and macOS, on both architectures, with Ruby 3.3, 3.4 and 4.0 extensions.
- **Fiber scheduler:** Three 100 ms loops in separate contexts and fibers finish in 100 ms vs 300 ms.
- **Upstream suites:** mini_racer passes 171 of 174 with 3 skipped by itself; ExecJS passes 197 of 198 with 1 skipped.
- **Difficult values:** A `Map` with a function key returns an error Hash instead of aborting with SIGABRT.
- **Checks:** Script-enforced crate boundaries, four fenced `unsafe` modules, loom, wire-codec fuzzing, nightly sanitizers and mini_racer differential tests.

These numbers compare with mini_racer 0.22.1. See [Benchmarks](#benchmarks) for the method.

## Install

Switch packages with one Gemfile line:

```diff
- gem "mini_racer"
+ gem "mini_racer_rs"
```

Run `bundle install`. Bundler is required because RubyGems may activate the original `mini_racer` first. Loading both gems raises a `LoadError` naming the conflict. Reverse the Gemfile change to switch back.

On x86_64-linux-gnu, aarch64-linux-gnu, x86_64-linux-musl, aarch64-linux-musl, arm64-darwin, and x86_64-darwin, `bundle install` installs a native gem without compiling. Elsewhere, the source gem requires Rust 1.91 or newer and libclang to build the extension. For a checkout, follow [CONTRIBUTING.md](CONTRIBUTING.md) and run `bin/setup`.

## Quick start

Both gems produced exactly these outputs:

```ruby check
require "mini_racer"

context = MiniRacer::Context.new(timeout: 500, max_memory: 50_000_000)

context.eval("function add(a, b) { return a + b }")
context.call("add", 2, 3)                      # => 5

context.attach("ruby.upcase", ->(text) { text.upcase })
context.eval("ruby.upcase('hello')")           # => "HELLO"

context.eval_await("Promise.resolve(42)")      # => 42

begin
  context.eval("while (true) {}")
rescue MiniRacer::ScriptTerminatedError => e
  e.message                                    # => "terminated"
end
context.eval("add(1, 1)")                      # => 2

snapshot = MiniRacer::Snapshot.new("var greeting = 'hello from a snapshot'")
MiniRacer::Context.new(snapshot: snapshot).eval("greeting") # => "hello from a snapshot"
```

ExecJS 2.10.2 uses it as its MiniRacer runtime:

```ruby check
require "execjs"

ExecJS.runtime.name                            # => "mini_racer (V8)"
ExecJS.eval("[1, 2].map(x => x * 2)")         # => [2, 4]
```

## API at a glance

| API | What it does |
|---|---|
| `Context.new(**options)` | Creates a context with its own V8 isolate and runner thread. |
| `Context#eval(source, filename: nil)` | Runs JavaScript and returns the result as Ruby values. |
| `Context#call(name, *args)` | Calls a global JavaScript function by dotted name with Ruby arguments. |
| `Context#eval_await`, `#call_await` | Like `eval` and `call`, but wait for a returned Promise to settle and return its value. A rejection raises `MiniRacer::RuntimeError`. |
| `Context#attach(name, callable)` | Exposes a Ruby proc or method under a dotted JavaScript name. |
| `Context#load(path)` | Evaluates a JavaScript file. |
| `Context#stop` | Terminates a running script from another thread. |
| `Context#dispose` | Frees the isolate. |
| `Context#heap_stats`, `#heap_snapshot`, `#write_heap_snapshot(path)`, `#low_memory_notification` | Inspects the heap, writes snapshots, or asks V8 to free memory. |
| `Context#pump_message_loop`, `#perform_microtask_checkpoint` | Runs pending V8 tasks and microtasks. |
| `Snapshot.new`, `#warmup!`, `#dump`, `Snapshot.load` | Creates, warms, serializes, and loads startup snapshots. |
| `Platform.set_flags!(*flags)` | Sets V8 flags before the first context. |
| `MiniRacerRs.register_source_map(bundle, map)` | Registers a source map for a bundle, so `MiniRacer::RuntimeError` backtraces show its original files and positions. |

`Context.new` accepts these options:

| Option | Effect |
|---|---|
| `timeout:` | Maximum milliseconds for each eval or call before `MiniRacer::ScriptTerminatedError`. |
| `max_memory:` | V8 heap limit in bytes, checked after each garbage collection. An overage raises `MiniRacer::V8OutOfMemoryError`, and the context remains usable. |
| `ensure_gc_after_idle:` | Idle milliseconds before one full garbage collection. `0` disables it. |
| `snapshot:` | A `MiniRacer::Snapshot` used to initialize the context. |

Errors are `MiniRacer::ParseError`, `MiniRacer::RuntimeError`, `MiniRacer::ScriptTerminatedError`, `MiniRacer::V8OutOfMemoryError`, `MiniRacer::ContextDisposedError`, `MiniRacer::SnapshotError`, and `MiniRacer::PlatformAlreadyInitialized`. `MiniRacer::RuntimeError` includes JavaScript frames in its backtrace. All descend from `MiniRacer::Error`.

**Source maps.** Bundles must be UTF-8 Strings of at least 1,024 bytes. Maps must be JSON Strings in source map v3 format. When a context evaluates UTF-8 source containing a registered bundle, mini_racer_rs rewrites `MiniRacer::RuntimeError` backtraces. V8 positions inside the bundle become mapped original file, line, and column positions. V8 function names remain.

V8 positions remain for async frames and frames after the tenth. They also remain for `eval`, `new Function`, uncovered positions, and frames outside the bundle. JavaScript-reformatted stacks also keep V8 positions.

Detailed stack capture makes creating and throwing errors more expensive in affected contexts. Measured React and MUI server renders were unchanged. Registration is process wide and holds at most eight bundles. Maps cannot be removed. Registering a bundle again replaces its map. Trust bundles and maps like application code because source names appear in Ruby backtraces.

## Switching from mini_racer

### What changes

The public API and require path stay the same. Changes include native installation, faster calls and Babel transforms, fiber-aware waiting, newer V8 behavior, and safer value conversion. `Object(1n)` returns `1`, and an offset typed array returns its buffer as a binary String. Applications making many small calls gain the most. See [Highlights](#highlights) for measurements.

### Check before you ship

- You're affected if tests or code compare exact JavaScript errors or locale-formatted text. V8 15.2 may differ from V8 13.6.

- You're affected if an attached callback reenters its context from another fiber, including inside `Enumerator#next`. mini_racer_rs raises `ThreadError`; mini_racer allows it.

- You're affected if a callback exits by `throw`, returns from a proc, or receives `Thread#kill`. The script ends with `Error: terminated`. The jump continues after `eval` returns. mini_racer hangs.

- You're affected if JavaScript reads a callback exception's non-ASCII message. mini_racer_rs passes UTF-8, such as `"café"`; mini_racer passes `"cafÃ©"`.

- You're affected if `fork`, `IO.popen("-")`, or `Process.daemon` runs after V8 starts. Both modes wait up to 500 ms for runners and interrupt JavaScript after 50 ms. They pause the worker pool and wait up to 100 ms for running tasks. Uninterruptible work may outlast these best-effort waits, and the fork proceeds. Default children reject inherited contexts. New contexts use the child's threads, but are best effort after either timeout. `:single_threaded` children take over idle inherited contexts and reject busy ones. After a runner timeout, they reject all inherited contexts. They accept new contexts.

- You're affected if `:single_threaded` forks bypass Ruby's `fork`, such as a C-level `fork()`. Inherited contexts are unusable in the child.

- You're affected if another thread calls `dispose` during a call. It returns after the call finishes and the isolate is gone. mini_racer returns immediately and frees the isolate later.

- You're affected if your code references `MiniRacer::Loader`. It does not exist in mini_racer_rs.

See [COMPATIBILITY.md](COMPATIBILITY.md) for every difference.

### Stay on mini_racer if

- Download size matters more than compile time. The largest native gem is 65.6 MB because it contains three Ruby extensions. mini_racer downloads 47.1 MB on x86_64 Linux, then compiles.

### Gems that depend on mini_racer

If another gem adds `mini_racer`, point Bundler to the repository's shim. This empty `mini_racer` gem is version 0.22.1. It depends on the `mini_racer_rs` version from the same checkout.

```ruby
gem "mini_racer_rs"
gem "mini_racer", github: "andymai/mini_racer_rs", tag: "vX.Y.Z", glob: "shim/*.gemspec"
```

The repository tests `path:` shim resolution with rtlcss in `test/consumers/transitive/`. The `github:` form has not run end to end. A dependency excluding `mini_racer` 0.22.1 cannot use the shim.

### Forking servers such as Puma

Before forking, evaluate each bundle of 1 MB or more twice in the master, then dispose those contexts. Use the renderer's exact source text and file name. The second evaluation builds the process-wide V8 code cache. Matching evaluations compile from it, and forked children inherit it. For Babel's 2.3 MB bundle at commit `5e13c1c`, a forked child's first load took 60.8 ms without a parent load. It took 65.5 ms after one load and 25.0 ms after two. These are medians across 5 trials. Prime a `react-rails` app by building its server renderer twice. Children do not use parent contexts. Only the cache carries over.

For Ruby's `fork`, `IO.popen("-")`, or `Process.daemon`, mini_racer_rs waits for context runners, pauses its worker pool and waits for running tasks. An async `WebAssembly.compile` in a new default-mode child completes on the child's own pool. On Ubuntu 24.04 with glibc 2.39, 20 of 20 mini_racer children aborted with signal 6 after React rendering; all mini_racer_rs children exited cleanly. On Fedora, mini_racer hung within 50 `test/stress/fork_busy.rb` forks; mini_racer_rs completed 300 per run on Fedora and Ubuntu.

With `mini_racer` 0.22.1, each worker compiles the bundle from source on its first load because it has no code cache.

## Benchmarks

| Workload | mini_racer 0.22.1 | mini_racer_rs | Ratio |
|---|---:|---:|---:|
| Babel transform of a 3 KB JSX component, through ExecJS | 10.1 ms | 5.2 ms | 2.0x |
| Terser minify of a 78 KB library, through ExecJS | 51.1 ms | 52.0 ms | 1.0x |
| React server render of 1,000 items (17 KB of HTML), through ExecJS | 191 µs | 174 µs | 1.1x |
| React in a new context: load the bundle and render 1,000 items, through ExecJS | 11.9 ms | 11.8 ms | 1.0x |
| Handlebars render of 50 rows, through `Context#call` | 49.1 µs | 34.9 µs | 1.4x |
| Autoprefixer on Bootstrap's 280 KB stylesheet, through ExecJS | 26.3 ms | 23.7 ms | 1.1x |
| MUI dashboard server render of 200 rows and 299 icons (469 KB of HTML), through ExecJS | 32.6 ms | 29.8 ms | 1.1x |
| MUI in a new context: load the 8.2 MB bundle and render once, through ExecJS | 167 ms | 105 ms | 1.6x |
| Load a 44 MB production server rendering bundle into a new context, through ExecJS | 983 ms | 215 ms | 4.6x |

Results are medians of 5 runs. Every workload produced identical output in both gems. The eight library workloads use the release build of commit `220bfe7`. The 44 MB production bundle load uses commit `2513a48`. Benchmarks ran on Ruby 3.4.2 and Linux with an AMD Ryzen 9 7950X3D. The governor was `performance`. Both gems were pinned to CPUs 0 through 7.

The per-call and memory figures below were measured on the same machine. Startup plus Babel, Babel context and MUI context figures come from commit `220bfe7`. One watchdog thread serves the whole process, making a call with `timeout:` fast. Scripts of 1 MB or more use a process-wide V8 code cache. Contexts loading the same large script share one source copy.

| Measure | mini_racer 0.22.1 | mini_racer_rs |
|---|---:|---:|
| eval round trip | 6.91 µs | 1.09 µs |
| call round trip | 6.51 µs | 0.72 µs |
| callback round trip | 6.12 µs | 1.12 µs |
| call on a context with `timeout:` | 20.48 µs | 0.76 µs |
| Babel inside V8 | 10.3 ms | 5.3 ms |
| startup plus Babel | 44.4 ms | 13.3 ms |
| context holding Babel | 28.5 MB | 15.2 MB |
| context holding the 8.2 MB MUI bundle | 110 MB | 44 MB |
| context holding the 44 MB production bundle, at commit `2513a48` before sources were shared | 301 MB | 136 MB |
| peak resident memory with 20,000 contexts left to Ruby's garbage collector | 2,920 MB | 102 MB |

```sh
bundle exec ruby bench/run.rb --cpus 0-7
```

[BENCHMARKS.md](BENCHMARKS.md) covers the method, per-call costs, memory results, and mini_racer's `benchmark/serde` suite.

<!-- compat-table:start -->
## Compatibility

| Engine | Done | Planned | Deviation |
|---|---|---|---|
| CRuby | 54 | 0 | 28 |
| TruffleRuby | 58 | 0 | 0 |

See [COMPATIBILITY.md](COMPATIBILITY.md).
<!-- compat-table:end -->

## How it works

mini_racer_rs uses the `v8` crate, rusty_v8 152.2.0, with magnus and rb-sys Ruby bindings. Each context owns a V8 isolate and native runner thread. Its stack is at least 2 MiB, keeping V8 frames off Ruby thread stacks. A per-context lock allows one dispatch at a time.

```text
 Ruby thread                          runner thread (one per context)
 ───────────                          ───────────────────────────────
 MiniRacer::Context#eval
   │ encode request (V8 wire format)
   ├──────────── channel ───────────▶ receive request
   │ spin, then park (GVL released)    run JavaScript in the V8 isolate
   │                                   serialize the result
   ◀──────────── channel ─────────────┤
   │ decode straight into Ruby objects
   ▼
 result
```

A waiting Ruby thread spins with the GVL held, then without it. It then parks with the GVL released. After enough consecutive answers arrive beyond the spin budget, that context skips spinning and parks at once until an answer arrives within the budget. Under a fiber scheduler, only the waiting fiber parks. Values use V8's serialization format and decode directly into Ruby objects. There is no intermediate tree. Repeated objects keep their identity.

One watchdog thread per process enforces every context's `timeout:`. In `:single_threaded` mode, forks quiesce runners. Children take over idle contexts; contexts in use raise there. mini_racer_rs gives V8 a copied platform vtable in each process. Worker-thread slots post to its own pool. V8's platform has one worker thread, which receives no tasks. Forking pauses the pool. The parent resumes it afterward. Default multi-threaded children reject inherited contexts.

The implementation has four crates:

- `mini_racer_wire`: the wire codec, with no unsafe code.
- `mini_racer_sync`: the thread protocol, with no unsafe code and loom checking.
- `mini_racer_engine`: V8 process state and runners, and the only crate naming `v8`.
- `mini_racer_rs`: the Ruby extension, and the only crate naming magnus or rb-sys.

The release pipeline runs the full suites for every released extension.

[ARCHITECTURE.md](ARCHITECTURE.md) follows a call end to end and explains the design.

## Scope

mini_racer_rs does not:

- Run on Windows. It supports glibc and musl Linux, including Alpine, on x86_64 and aarch64, and macOS on arm64 and x86_64.

- Provide Node.js APIs. JavaScript runs in plain V8 without file system, network, or process access. It can reach only Ruby callables you `attach`.

- Work reliably without Bundler. RubyGems may activate the original mini_racer first.

- Satisfy dependencies pinning `mini_racer` versions other than 0.22.1. This includes minibars and its `~> 0.6.4` requirement.

- Extend the `MiniRacer` API beyond mini_racer 0.22.1. [COMPATIBILITY.md](COMPATIBILITY.md) records deliberate differences.

Supported implementations are CRuby 3.3, 3.4, and 4.0, plus TruffleRuby through mini_racer's unchanged TruffleRuby backend.

## Documentation

| Document | Covers |
|---|---|
| [ARCHITECTURE.md](ARCHITECTURE.md) | Crates, a call end to end, values, termination, callbacks, promises, snapshots, forks, fiber schedulers, safety rules, and verification. |
| [COMPATIBILITY.md](COMPATIBILITY.md) | Every public API item and deliberate difference from mini_racer 0.22.1, with its reason. |
| [BENCHMARKS.md](BENCHMARKS.md) | Benchmark method and full results. |
| [STABILITY.md](STABILITY.md) | Versioning, the public API, unstable interfaces, and support policy. |
| [SECURITY.md](SECURITY.md) | Vulnerability reporting, untrusted JavaScript, and supply-chain controls. |
| [CONTRIBUTING.md](CONTRIBUTING.md) | Setup, checks, tests, and conventions. |

## License

mini_racer_rs is available under the [MIT License](LICENSE). [AI-DISCLOSURE.md](AI-DISCLOSURE.md) describes how AI tooling was used to build the project.