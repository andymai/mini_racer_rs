use magnus::value::ReprValue;
use magnus::{Error, Object, RHash, RModule, Ruby};
use mini_racer_engine::process::fork;

use crate::context::Context;
use crate::export;

pub fn init(rs: RModule) -> Result<(), Error> {
    rs.define_singleton_method("counters", export!(function counters, 1))?;
    rs.define_singleton_method("code_cache", export!(function code_cache, 0))?;
    Ok(())
}

// Read from atomics alone, so a forked child reads the copy it inherited
// without a lock a thread of its parent may have held at the fork.
fn code_cache(ruby: &Ruby) -> Result<RHash, Error> {
    let generation = fork::current();
    let (cache, sources) = (generation.code_cache(), generation.sources());
    let hash = ruby.hash_new();
    for (name, count) in [
        ("entries", cache.entries()),
        ("bytes", cache.bytes()),
        ("hits", cache.hits()),
        ("builds", cache.stored()),
        ("rebuilds", cache.rebuilt()),
        ("refused", cache.refusals()),
        ("shared_sources", sources.kept()),
        ("shared_bytes", sources.kept_bytes()),
    ] {
        hash.aset(ruby.to_symbol(name), count)?;
    }
    hash.freeze();
    Ok(hash)
}

// Read without the context's lock, so a thread can read a context another
// thread is running.
fn counters(ruby: &Ruby, context: &Context) -> Result<RHash, Error> {
    let counts = context.counters(ruby)?;
    let terminated = counts.terminated;
    let causes = ruby.hash_new();
    for (name, count) in [
        ("stop", terminated.stop),
        ("timeout", terminated.timeout),
        ("interrupt", terminated.interrupt),
        ("max_memory", terminated.max_memory),
        ("heap_exhausted", terminated.heap_exhausted),
    ] {
        causes.aset(ruby.to_symbol(name), count)?;
    }
    let hash = ruby.hash_new();
    for (name, count) in [
        ("dispatches", counts.dispatches),
        ("callbacks", counts.callbacks),
        ("bytes_to_js", counts.bytes_to_js),
        ("bytes_from_js", counts.bytes_from_js),
        ("js_ns", counts.js_ns),
        ("gc_count", counts.gc_count),
        ("gc_ns", counts.gc_ns),
    ] {
        hash.aset(ruby.to_symbol(name), count)?;
    }
    hash.aset(ruby.to_symbol("terminated"), causes)?;
    Ok(hash)
}
