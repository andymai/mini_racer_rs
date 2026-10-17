# mini_racer_rs

A Rust implementation of the MiniRacer Ruby API, built as a drop-in replacement for mini_racer 0.22.1.

## Status

Pre-release and under active development. Nothing is published to RubyGems.

This independent project is not affiliated with the mini_racer maintainers. Compatibility targets mini_racer's test suite at v0.22.1, run unmodified.

## Switching from mini_racer

### 1. Swap the gem

Under Bundler, replace `mini_racer` with `mini_racer_rs`:

```diff
- gem "mini_racer"
+ gem "mini_racer_rs"
```

Bundler is required because RubyGems may otherwise activate the original `mini_racer` gem first.

`mini_racer_rs` defines the `MiniRacer` namespace and ships `lib/mini_racer.rb`, so existing requires and ExecJS integrations keep working:

```ruby
require "mini_racer"

context = MiniRacer::Context.new
context.eval("1 + 1") # => 2
```

If another gem in your bundle depends on `mini_racer`, see [Gems that depend on mini_racer](#gems-that-depend-on-mini_racer) below for the shim recipe. To switch back, reverse the one-line Gemfile change.

### 2. What you'll notice

Install behavior changes. `mini_racer` 0.22.1 is published only as a source gem, so `bundle install` compiles its C++ extension against libv8-node’s prebuilt V8 and requires a C++ toolchain. The `mini_racer_rs` release workflow builds native gems for glibc Linux on x86_64 and aarch64, and macOS on arm64 and x86_64. Each contains extensions for Ruby 3.3, 3.4, and 4.0, so installation compiles nothing.

Speed depends on the workload. An eval, call, or callback round trip takes 1.3 to 1.6 µs, compared with 6.1 to 6.6 µs in `mini_racer`. A call on a context created with `timeout:` takes 1.6 µs instead of 20.7 µs because `mini_racer` starts a watchdog thread for every timed call, while `mini_racer_rs` uses one per process.

In the real workloads below, a Babel transform through ExecJS is 1.9x faster, because Babel's own JavaScript runs faster in V8 15.2, and a Handlebars render is 1.2x faster. Terser minification and React rendering are about the same because most of their time is spent running JavaScript inside V8. Apps making many small calls gain the most.

Under a fiber scheduler such as `async`, a fiber waiting on JavaScript lets other fibers on the thread run. `mini_racer` blocks the thread. Three 100 ms busy loops in separate contexts and fibers finish in 100 ms, compared with 300 ms.

Some values that make `mini_racer` abort or raise are returned. A `Map` with a function key aborts the `mini_racer` process with `SIGABRT`, but returns an error Hash here. `Object(1n)` returns `1`, and a typed array at a byte offset returns its buffer as a binary String. `mini_racer` raises `MiniRacer::RuntimeError` for the latter two.

The bundled V8 version is 15.2.124.1, compared with 13.6.233.17 in `mini_racer` 0.22.1.

### 3. Check before you ship

- You’re affected if your tests or code compare exact JavaScript errors or locale-formatted text. V8-generated wording can differ between versions 15.2 and 13.6.

- You’re affected if an attached Ruby callback calls back into its own context from another fiber, such as inside `Enumerator#next`. `mini_racer_rs` raises `ThreadError`; `mini_racer` allows it.

- You’re affected if a callback exits by `throw`, returns out of a proc, or is stopped by `Thread#kill`. The script ends with `Error: terminated`, then the jump continues after `eval` returns; `mini_racer` hangs.

- You’re affected if JavaScript reads a non-ASCII message from a callback exception. `mini_racer_rs` passes UTF-8 (`"café"`), while `mini_racer` passes `"cafÃ©"`.

- You’re affected if you fork with `:single_threaded` while another thread runs JavaScript V8 cannot interrupt, such as a large `JSON.parse`. The fork waits up to 500 ms, then the child refuses inherited contexts; new contexts work.

- You’re also affected if, with `:single_threaded`, you fork in a way that bypasses Ruby’s `fork`, such as a C-level `fork()`. Inherited contexts are unusable in the child.

- You’re affected if another thread calls `dispose` while a call is running. It returns after the call finishes and the isolate is gone; `mini_racer` returns immediately and frees the isolate later.

- You’re affected if your code references `MiniRacer::Loader`. It does not exist in `mini_racer_rs`.

See [COMPATIBILITY.md](COMPATIBILITY.md) for every behavioral difference.

### 4. Stay on mini_racer if

- You deploy on musl-based Linux such as Alpine. `mini_racer` supports it through libv8-node’s musl gems; `mini_racer_rs` does not install there. Neither gem supports Windows.

- You need a gem from RubyGems today. `mini_racer_rs` is pre-release, while `mini_racer` is the established gem maintained by Discourse staff.

- Download size matters more than compile time. The largest `mini_racer_rs` native gem is 65.6 MB because it contains extensions for three Ruby versions. The `mini_racer` download on x86_64 Linux is 47.1 MB, followed by compilation.

## Gems that depend on mini_racer

If your application gets `mini_racer` through another gem, point Bundler at the repository's shim. The shim is an empty gem named `mini_racer`, version 0.22.1, that depends on the `mini_racer_rs` version from the same checkout.

```ruby
gem "mini_racer_rs"
gem "mini_racer", github: "andymai/mini_racer_rs", tag: "vX.Y.Z", glob: "shim/*.gemspec"
```

Bundler is required because RubyGems may activate the original `mini_racer` first. The shim lets a transitive dependency resolve to `mini_racer_rs`, which provides `MiniRacer` and `lib/mini_racer.rb`. Existing `require "mini_racer"` calls and ExecJS use remain unchanged.

The repository tests shim resolution with `path:` sources and `rtlcss` in `test/consumers/transitive/`. The `github:` form has not been run end to end. A gem whose `mini_racer` requirement does not accept version 0.22.1 cannot use the shim. Loading both implementations raises a `LoadError` naming the conflict.

## Scope

Supported platforms are Linux with glibc on x86_64 and aarch64, and macOS on arm64 and x86_64. Supported Rubies are CRuby 3.3, 3.4, and 4.0, and TruffleRuby.

musl-based Linux, including Alpine, and Windows are out of scope.

<!-- compat-table:start -->
## Compatibility

| Engine | Done | Planned | Deviation |
|---|---|---|---|
| CRuby | 54 | 0 | 22 |
| TruffleRuby | 58 | 0 | 0 |

See [COMPATIBILITY.md](COMPATIBILITY.md).
<!-- compat-table:end -->

## Benchmarks

Both gems ran on Ruby 3.4.2 and an AMD Ryzen 9 7950X3D under Linux, pinned to the same eight cores with the `performance` governor. These are medians of nine runs, with each workload in its own process, using a release build of commit `bc492d3`; the run verifies identical output for every workload.

| Workload | mini_racer 0.22.1 | mini_racer_rs | Ratio |
|---|---:|---:|---:|
| Babel transform of a 3 KB JSX component, through ExecJS | 9.9 ms | 5.2 ms | 1.9x |
| Handlebars render of 50 table rows, through `Context#call` | 49.5 µs | 41.9 µs | 1.2x |
| React server render of 1,000 list items (17 KB of HTML), through ExecJS | 193 µs | 189 µs | 1.0x |
| Terser minify of a 78 KB library, through ExecJS | 52.2 ms | 51.5 ms | 1.0x |
| Load the React bundle into a new context and render 1,000 items, through ExecJS | 11.9 ms | 11.9 ms | 1.0x |

Babel's own JavaScript runs faster in V8 15.2: timed inside the engine, one transform takes 5.3 ms, against 10.3 ms in mini_racer's V8 13.6. Terser and React spend nearly all their time running JavaScript and take about the same time in both.

Reproduce the run with `bundle exec ruby bench/run.rb --cpus 0-7`; results depend on the machine. See [BENCHMARKS.md](BENCHMARKS.md) for the full method, per-call measurements, and results from `mini_racer`’s own suite.

## How it works

mini_racer_rs uses Rust, V8 15.2 through the `v8` crate (rusty_v8), and Ruby bindings through magnus and rb-sys. Each `MiniRacer::Context` runs V8 on its own native thread.

When a Fiber scheduler is active, waiting on a context yields to other fibers instead of blocking the thread.

## More

[`SECURITY.md`](SECURITY.md) covers vulnerability reporting, runtime boundaries, and supply chain controls.

[`STABILITY.md`](STABILITY.md) covers versioning, the public API, unstable interfaces, and support policies.

[`CONTRIBUTING.md`](CONTRIBUTING.md) covers setup, checks, tests, and contribution conventions.

[`COMPATIBILITY.md`](COMPATIBILITY.md) covers API status and deliberate differences from `mini_racer` 0.22.1.

## License

MIT. See `AI-DISCLOSURE.md` for how AI tooling was used to build the project.
