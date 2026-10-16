# mini_racer_rs

A Rust implementation of the MiniRacer Ruby API, built as a drop-in replacement for mini_racer 0.22.1.

## Status

Pre-release and under active development. Nothing is published to RubyGems.

This independent project is not affiliated with the mini_racer maintainers. Compatibility targets mini_racer's test suite at v0.22.1, run unmodified.

## Usage

Under Bundler, replace:

```diff
-gem "mini_racer"
+gem "mini_racer_rs"
```

Bundler is required because RubyGems may otherwise activate the original mini_racer gem first. mini_racer_rs defines the `MiniRacer` namespace and ships `lib/mini_racer.rb`, so `require "mini_racer"` and ExecJS work unchanged.

```ruby
require "mini_racer"
context = MiniRacer::Context.new
context.eval("1 + 1") # => 2
```

## Scope

Supported platforms are Linux with glibc on x86_64 and aarch64, and macOS on arm64 and x86_64. Supported Rubies are CRuby 3.3, 3.4, and 4.0, and TruffleRuby.

musl-based Linux, including Alpine, and Windows are out of scope.

<!-- compat-table:start -->
## Compatibility

| Engine | Done | Planned | Deviation |
|---|---|---|---|
| CRuby | 42 | 12 | 14 |
| TruffleRuby | 58 | 0 | 0 |

See [COMPATIBILITY.md](COMPATIBILITY.md).
<!-- compat-table:end -->

## How it works

mini_racer_rs uses Rust, V8 15.2 through the `v8` crate (rusty_v8), and Ruby bindings through magnus and rb-sys. Each `MiniRacer::Context` runs V8 on its own native thread.

When a Fiber scheduler is active, waiting on a context yields to other fibers instead of blocking the thread.

## License

MIT. See `AI-DISCLOSURE.md` for how AI tooling was used to build the project.
