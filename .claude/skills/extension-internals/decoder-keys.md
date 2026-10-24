# Hash keys and bulk inserts

Code: `ext/mini_racer_rs/src/decode.rs` (`interned`, `KeyCache`, `Pairs`, `put_pair`); `ext/mini_racer_rs/src/rb_raw.rs` (`interned_utf8`, `plain_key`); `crates/mini_racer_wire/src/decode.rs` (`Visitor::key`). Tests: `test/ruby/result_keys_test.rb`, against mini_racer's keys.

`ARCHITECTURE.md` "Values" and the comments on those symbols cover the interned keys, json 3.1's key cache for objects inside arrays, one bulk insert per hash, and the `gc_root::Spill` past 128 values. Past those:

- A decoded hash takes its pairs when it ends, in one `rb_hash_bulk_insert` into its still empty table (`Pairs`), which keeps a repeated key as `Hash#[]=` does: the first key's place and object with the last value.
- A key that is not a String of class String, an Integer, a Float, a Symbol, nil, true or false (`rb_raw::plain_key`), such as a Hash or an Array that refers back to an open hash, first puts every open hash's waiting pairs in, so its `hash` and `eql?` see each one as mini_racer has built it by then, and goes in by itself; a hash that already holds pairs takes the rest one by one, since a bulk insert would rehash it whole.
- A key with a lone surrogate stays a UTF-16LE String for the hash to intern, the function marker stays a `JavaScriptFunction` (an object's key is then its `to_s`), and a numeric key still goes through `to_s`; keys of arrays, which are dropped, and an error's fields are made as before.
