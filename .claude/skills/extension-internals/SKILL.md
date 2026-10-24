---
name: extension-internals
description: Use when changing or debugging ext/mini_racer_rs's encoding of Ruby Strings (encode.rs, rb_raw::cached_ascii, Transcoder), the copy of a large eval source without the GVL (context.rs source_text, rb_raw::read_without_gvl), the decoder's hash keys and bulk inserts (decode.rs, Pairs, KeyCache, plain_key), its stack checks (check_stack, SystemStackError on a small fiber or thread stack), its scratch buffer for Latin-1 and UTF-16 text, MiniRacerRs::Pool (pool.rs: checkouts, checkins, replacements, forks, shutdown), or MiniRacerRs.timeout scopes (timeout.rs); or when a call with long or non-ASCII Strings is slow, a result's keys differ from mini_racer's, a deep result faults or raises SystemStackError, a pool checkout hangs or misses its wakeup, or a timeout scope reaches a thread it should not.
---

# Extension internals

What the Ruby extension does past `ARCHITECTURE.md`, and why. Before changing an area's code, read its file; the others can stay unread.

| Area | Code | Read |
|---|---|---|
| Strings the encoder sends | `ext/mini_racer_rs/src/encode.rs` | [encoder-strings.md](encoder-strings.md) |
| Large eval sources copied without the GVL | `ext/mini_racer_rs/src/context.rs`, `ext/mini_racer_rs/src/rb_raw.rs` | [eval-sources.md](eval-sources.md) |
| Hash keys and bulk inserts in the decoder | `ext/mini_racer_rs/src/decode.rs` | [decoder-keys.md](decoder-keys.md) |
| The decoder's stack checks | `ext/mini_racer_rs/src/decode.rs` | [decoder-stack.md](decoder-stack.md) |
| The decoder's scratch buffer for text | `ext/mini_racer_rs/src/decode.rs` | [scratch-buffer.md](scratch-buffer.md) |
| `MiniRacerRs::Pool` | `ext/mini_racer_rs/src/pool.rs` | [pool.md](pool.md) |
| `MiniRacerRs.timeout` scopes | `ext/mini_racer_rs/src/timeout.rs` | [timeout-scope.md](timeout-scope.md) |
