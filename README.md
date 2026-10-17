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

mini_racer_rs implements the `MiniRacer` Ruby API from mini_racer 0.22.1, with faster Ruby and JavaScript round trips, V8 15.2, prebuilt native gems, fiber scheduler support, and safer handling of difficult values. It defines the `MiniRacer` namespace and ships `lib/mini_racer.rb`, so `require "mini_racer"` and ExecJS continue to work.

> **Pre-release:** Nothing is published to RubyGems.org yet, and the release workflow is held. Install from the repository until the first release. mini_racer_rs is an independent project and is not affiliated with the Discourse staff who maintain mini_racer.

## Highlights

- **4 to 5 times faster Ruby round trips.** An eval, call, or callback round trip takes 1.3 to 1.6 µs, against 6.1 to 6.6 µs in mini_racer. A call on a context with `timeout:` takes 1.6 µs instead of 20.7 µs because one watchdog serves the process.

- **1.9 times faster Babel transforms.** A transform through ExecJS takes 5.2 ms instead of 9.9 ms. Babel itself takes 5.3 ms inside V8 15.2, against 10.3 ms inside V8 13.6. Terser and React rendering take about the same time in both gems.

- **V8 15.2.124.1.** mini_racer 0.22.1 uses V8 13.6.233.17.

- **No compiler at release install.** Native gems cover glibc Linux on x86_64 and aarch64, plus macOS on arm64 and x86_64. Each carries extensions for Ruby 3.3, 3.4, and 4.0. mini_racer 0.22.1 ships as source and compiles C++ against libv8-node.

- **Fiber scheduler support.** A fiber waiting on JavaScript lets other fibers on its thread run. Three 100 ms busy loops in separate contexts and fibers finish in 100 ms, against 300 ms when mini_racer blocks the thread.

- **171 of 174 upstream tests pass.** The mini_racer suite runs unmodified, with its other 3 tests skipped by mini_racer itself. ExecJS passes 197 of 198 tests on its MiniRacer runtime, with 1 skipped.

- **Difficult values return safely.** A `Map` with a function key returns an error Hash instead of aborting the process with SIGABRT. `Object(1n)` returns `1`, and an offset typed array returns its buffer as a binary String.

- **Four checked layers.** A script enforces crate boundaries, `unsafe` is confined to four fenced modules, loom model-checks the thread handoff, fuzzing covers the wire codec, sanitizers run nightly, and results are differentially tested against mini_racer. The release pipeline runs the full suites for every released extension.

All performance numbers compare mini_racer_rs with mini_racer 0.22.1 on Ruby 3.4.2 and Linux on an AMD Ryzen 9 7950X3D, with both gems pinned to the same eight cores and the `performance` governor.

## Install

The package switch is one Gemfile line:

```diff
- gem "mini_racer"
+ gem "mini_racer_rs"
```

Run `bundle install`. Bundler is required because RubyGems may otherwise activate the original `mini_racer` first. Loading both gems raises a `LoadError` that names the conflict. Reverse the Gemfile change to switch back.

Until the first release publishes native gems, build from a checkout as described in [CONTRIBUTING.md](CONTRIBUTING.md). Run `bin/setup`. Source builds require Rust 1.91 or newer and libclang. Once published, installing a native gem will compile nothing.

## Quick start

This example was run against both gems and produced exactly the outputs shown:

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

`Context.new` accepts these options:

| Option | Effect |
|---|---|
| `timeout:` | Maximum milliseconds for each eval or call before `MiniRacer::ScriptTerminatedError`. |
| `max_memory:` | V8 heap limit in bytes, checked after each garbage collection. An overage raises `MiniRacer::V8OutOfMemoryError`, and the context remains usable. |
| `ensure_gc_after_idle:` | Idle milliseconds before one full garbage collection. `0` disables it. |
| `snapshot:` | A `MiniRacer::Snapshot` used to initialize the context. |

Errors are `MiniRacer::ParseError`, `MiniRacer::RuntimeError` with JavaScript frames in its backtrace, `MiniRacer::ScriptTerminatedError`, `MiniRacer::V8OutOfMemoryError`, `MiniRacer::ContextDisposedError`, `MiniRacer::SnapshotError`, and `MiniRacer::PlatformAlreadyInitialized`. All descend from `MiniRacer::Error`.

## Switching from mini_racer

### What changes

The public API and require path stay the same. The main changes are native installation, faster small calls and Babel transforms, fiber-aware waiting, newer V8 behavior, and safer conversion of difficult values. Applications that make many small calls gain the most. See [Highlights](#highlights) for measurements.

### Check before you ship

- You're affected if your tests or code compare exact JavaScript error messages or locale-formatted text. V8 15.2 can word them differently from V8 13.6.

- You're affected if an attached Ruby callback calls back into its own context from another fiber, such as inside `Enumerator#next`. mini_racer_rs raises `ThreadError`; mini_racer allows it.

- You're affected if a callback exits by `throw`, returns out of a proc, or is stopped by `Thread#kill`. The script ends with `Error: terminated`, then the jump continues after `eval` returns. mini_racer hangs.

- You're affected if JavaScript reads a non-ASCII message from a callback exception. mini_racer_rs passes UTF-8, such as `"café"`, while mini_racer passes `"cafÃ©"`.

- You're affected if you fork while another thread runs JavaScript, especially uninterruptible work such as a large `JSON.parse`. Both modes wait up to 500 ms for runners; the default also waits for V8's worker threads to sleep. After a timeout, `:single_threaded` children refuse every inherited context, but new contexts work. Default-mode children always refuse inherited contexts, and new contexts are best effort.

- You're affected if, with `:single_threaded`, you fork in a way that bypasses Ruby's `fork`, such as a C-level `fork()`. Inherited contexts are unusable in the child.

- You're affected if another thread calls `dispose` while a call is running. It returns after the call finishes and the isolate is gone. mini_racer returns immediately and frees the isolate later.

- You're affected if your code references `MiniRacer::Loader`. It does not exist in mini_racer_rs.

See [COMPATIBILITY.md](COMPATIBILITY.md) for every difference.

### Stay on mini_racer if

- You deploy on musl-based Linux such as Alpine. mini_racer supports it through libv8-node's musl gems, while mini_racer_rs does not install there. Neither gem supports Windows.

- You need a gem from RubyGems.org today. mini_racer_rs is pre-release, while mini_racer is the established gem maintained by Discourse staff.

- Download size matters more than compile time. The largest mini_racer_rs native gem is 65.6 MB because it includes extensions for three Ruby versions. The mini_racer download on x86_64 Linux is 47.1 MB, followed by compilation.

### Gems that depend on mini_racer

If another gem brings in `mini_racer`, point Bundler at the repository's shim. It is an empty gem named `mini_racer`, version 0.22.1, that depends on the `mini_racer_rs` version from the same checkout.

```ruby
gem "mini_racer_rs"
gem "mini_racer", github: "andymai/mini_racer_rs", tag: "vX.Y.Z", glob: "shim/*.gemspec"
```

The repository tests shim resolution with `path:` sources and rtlcss in `test/consumers/transitive/`. The `github:` form has not been run end to end. A dependency whose `mini_racer` requirement excludes version 0.22.1 cannot use the shim.

## Benchmarks

| Workload | mini_racer 0.22.1 | mini_racer_rs | Ratio |
|---|---:|---:|---:|
| Babel transform of a 3 KB JSX component, through ExecJS | 9.9 ms | 5.2 ms | 1.9x |
| Handlebars render of 50 table rows, through `Context#call` | 49.5 µs | 41.9 µs | 1.2x |
| React server render of 1,000 list items (17 KB of HTML), through ExecJS | 193 µs | 189 µs | 1.0x |
| Terser minify of a 78 KB library, through ExecJS | 52.2 ms | 51.5 ms | 1.0x |
| Load the React bundle into a new context and render 1,000 items, through ExecJS | 11.9 ms | 11.9 ms | 1.0x |

These are medians of nine runs. Each workload ran in its own process, and every workload produced identical output. Measurements use the release build of commit `bc492d3` on Ruby 3.4.2 and Linux on an AMD Ryzen 9 7950X3D, with both gems pinned to the same eight cores and the `performance` governor.

Reproduce them with:

```sh
bundle exec ruby bench/run.rb --cpus 0-7
```

Results depend on the machine. [BENCHMARKS.md](BENCHMARKS.md) contains the full method, per-call measurements, and mini_racer's own `benchmark/serde` suite.

<!-- compat-table:start -->
## Compatibility

| Engine | Done | Planned | Deviation |
|---|---|---|---|
| CRuby | 54 | 0 | 23 |
| TruffleRuby | 58 | 0 | 0 |

See [COMPATIBILITY.md](COMPATIBILITY.md).
<!-- compat-table:end -->

## How it works

mini_racer_rs uses the `v8` crate, rusty_v8 152.2.0, with Ruby bindings through magnus and rb-sys. Each context owns a V8 isolate and a native runner thread with a stack of at least 2 MiB, so V8 frames never use a Ruby thread's stack. A per-context lock permits one dispatch at a time.

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

A waiting Ruby thread first spins briefly with the GVL held, then spins without it, then parks with the GVL released. Under a fiber scheduler, only the waiting fiber parks. Values use V8's serialization format and decode directly into Ruby objects without an intermediate tree. Repeated objects retain their identity.

One watchdog thread per process enforces every context's `timeout:`. In `:single_threaded` mode, a fork with idle contexts quiesces the runners, and the child takes those contexts over; in the default multi-threaded mode, the fork also waits for V8's worker threads to be observed asleep, while inherited contexts remain unavailable in the child.

The implementation is split into four crates:

- `mini_racer_wire`: the wire codec, with no unsafe code.
- `mini_racer_sync`: the thread protocol, with no unsafe code and loom checking.
- `mini_racer_engine`: V8 process state and runners, and the only crate that names `v8`.
- `mini_racer_rs`: the Ruby extension, and the only crate that names magnus or rb-sys.

[ARCHITECTURE.md](ARCHITECTURE.md) follows a call end to end and covers the design in depth.

## Scope

mini_racer_rs deliberately does not:

- Run on Windows or musl-based Linux such as Alpine. Supported platforms are glibc Linux on x86_64 and aarch64, plus macOS on arm64 and x86_64.

- Provide Node.js APIs. JavaScript runs in plain V8 without file system, network, or process access. JavaScript can reach only the Ruby callables you `attach`.

- Work reliably without Bundler. RubyGems may activate the original mini_racer first.

- Satisfy a dependency that pins a `mini_racer` version other than 0.22.1. This includes minibars with its `~> 0.6.4` requirement.

- Extend the `MiniRacer` API beyond mini_racer 0.22.1. Deliberate differences are recorded in [COMPATIBILITY.md](COMPATIBILITY.md).

Supported Ruby implementations are CRuby 3.3, 3.4, and 4.0, plus TruffleRuby through mini_racer's unchanged TruffleRuby backend.

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