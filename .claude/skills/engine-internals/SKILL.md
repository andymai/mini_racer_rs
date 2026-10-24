---
name: engine-internals
description: Use when changing or debugging crates/mini_racer_engine's code cache (process/code_cache.rs), shared large sources (process/sources.rs, external strings), fast calls (fast_calls.rs), a call's arguments built by the runner (request.rs), primitive results (serialize.rs), startup snapshots and their blob (snapshot.rs), source maps (crates/mini_racer_sourcemap, process/source_maps.rs), MiniRacerRs.heap_limit, MiniRacerRs.counters, forks and V8's worker tasks (process/fork.rs, process/workers.rs), or the watchdog; or when a cache entry is missed or never rebuilt, resident memory grows after large loads or context churn, a heap limit aborts the process or raises where it should not, a snapshot blob is refused or V8 aborts on one, a frame is not rewritten through its source map, a counter loses increments, or a forked child hangs or aborts.
---

# Engine internals

What the engine does past `ARCHITECTURE.md`, and why. Before changing an area's code, read its file; the others can stay unread.

| Area | Code | Read |
|---|---|---|
| Code cache for evals of 128 KiB or more | `crates/mini_racer_engine/src/process/code_cache.rs` | [code-cache.md](code-cache.md) |
| One shared copy of each large source | `crates/mini_racer_engine/src/process/sources.rs` | [shared-sources.md](shared-sources.md) |
| Fast calls for ExecJS calls and react-rails renders | `crates/mini_racer_engine/src/fast_calls.rs` | `ARCHITECTURE.md` "Fast calls", the `behavior:fast-calls@ruby` reason in `compat.toml`, and the comments in `crates/mini_racer_engine/src/fast_calls.rs` and on `runner::fast_call` |
| A call's arguments built by the runner | `crates/mini_racer_engine/src/request.rs` | [call-arguments.md](call-arguments.md) |
| Primitive results written without V8's serializer | `crates/mini_racer_engine/src/serialize.rs` | `ARCHITECTURE.md` "Values", v8-bump step 14, and the comments on `primitive` in `crates/mini_racer_engine/src/serialize.rs` and `is_smi` in `crates/mini_racer_engine/src/sys.rs` |
| Startup snapshots and the blob their contexts share | `crates/mini_racer_engine/src/snapshot.rs` | [snapshots.md](snapshots.md) |
| Source maps: parsing, the registry, matching, rewriting | `crates/mini_racer_sourcemap`, `crates/mini_racer_engine/src/process/source_maps.rs` | [source-maps.md](source-maps.md) |
| `MiniRacerRs.heap_limit` | `crates/mini_racer_engine/src/runner.rs` | [heap-limit.md](heap-limit.md) |
| `MiniRacerRs.counters` | `crates/mini_racer_engine/src/counters.rs` | [counters.md](counters.md) |
| Forks and V8's worker tasks | `crates/mini_racer_engine/src/process/fork.rs`, `crates/mini_racer_engine/src/process/workers.rs` | [forks.md](forks.md) |
| The watchdog and `Weak` references to contexts | `crates/mini_racer_engine/src/process/watchdog.rs` | [watchdog.md](watchdog.md) |
