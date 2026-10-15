# mini_racer_rs review rules

Scripts already enforce layering, unsafe fences, FFI guards, GC roots, statics, exports,
version pins, action pins and prose (`scripts/check-*.sh`). Review for what they cannot see:

- Ruby under `lib/` stays vocabulary; behavior lives in Rust.
- Vendored upstream files and `vendor/` submodules are never edited.
- Intra-gem loads use `require_relative`.
- Behavior that differs from mini_racer 0.22.1 carries a `compat.toml` deviation and a failing-first fixture.
- Comments explain a non-obvious why and nothing else.
