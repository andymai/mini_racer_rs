---
name: layer-boundaries
description: Use when adding a dependency to any crate, deciding which crate new code belongs in, writing unsafe code or an FFI entry point, keeping Ruby values in Rust, adding process-global state, or fixing a VIOLATION from a scripts/check-*.sh script or the CI Boundaries job.
---

# Layer Boundaries

The workspace is a strict DAG: L0 `mini_racer_wire` and `mini_racer_sync`, L1 `mini_racer_engine`, L2 `mini_racer_rs` (the Ruby extension). `layers.toml` holds the allowed dependencies and the fenced `unsafe` modules; the table in `CLAUDE.md` mirrors it.

## Quick reference

```bash
scripts/check-boundaries.sh     # dependency graph against layers.toml
scripts/check-unsafe-fences.sh  # unsafe_code exemptions only on fenced modules
scripts/check-ffi-guards.sh     # export! and panic guards
scripts/check-gc-roots.sh       # Ruby values held in Rust go through gc_root
scripts/check-statics.sh        # engine statics only in engine::process
scripts/test-boundaries.sh      # plants one violation per rule and expects each to fail
```

## Where new code goes

| Code | Crate |
|---|---|
| Parsing or writing V8's serialization format, Ruby-free | `mini_racer_wire` |
| Handoff, termination and fork protocol types (loom-checked), Ruby-free | `mini_racer_sync` |
| Anything that calls `v8`, owns isolates, threads, or process state | `mini_racer_engine` |
| Anything that touches Ruby (magnus, rb-sys, VALUEs, the GVL) | `mini_racer_rs` |

The engine never calls Ruby; the extension never names `v8` or `mini_racer_sync` directly.

## Procedures

**Adding a dependency.** Add it to the crate's `Cargo.toml` and to that crate's `normal` list in `layers.toml`, then run `scripts/check-boundaries.sh`. A Ruby crate (`magnus`, `rb-sys`) reachable from L0 or L1, even transitively, fails; move the code up a layer instead.

**Writing unsafe code.** Put it in a fenced module (`engine::sys`, `ext::rb_raw`, `ext::guard`, `ext::gc_root`), one operation per `unsafe` block, each with a `// SAFETY:` comment. A new fenced module needs an entry in `unsafe_modules` in `layers.toml` and an `#[allow(unsafe_code, reason = "...")]` on its `mod` line in the crate's lib.rs, nowhere else.

**Adding a Ruby method or an extern "C" fn.** Define methods through `export!` in `ext/mini_racer_rs/src/lib.rs`. Every `extern "C"` body calls `guard()` (extension) or `catch_unwind` (engine).

**Keeping a Ruby value in Rust.** Use `gc_root::StaticRoot` (or the collection types `gc_root` gains later); never a bare `Lazy`, `Opaque`, `BoxValue`, or `rb_gc_register_*`.

**Adding process-global state.** Declare it under `crates/mini_racer_engine/src/process.rs` or `crates/mini_racer_engine/src/process/`, so fork reconciliation can find it.

**Adding or changing a check rule.** Add a planted case to `scripts/test-boundaries.sh` that fails for that rule's tag, and confirm it fails before the fix and passes after.

Tag catalog and fixes: [reference.md](reference.md).
