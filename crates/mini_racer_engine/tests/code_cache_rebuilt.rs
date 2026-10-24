//! The context that cached a script builds its cache again once, after its
//! next call, so the cache holds the functions that call compiled; in a file
//! of its own, as the cache is the process's.

#![allow(
    clippy::expect_used,
    reason = "test helpers fail the test by panicking"
)]

use std::fmt::Write;

use mini_racer_engine::process::code_cache::CodeCache;
use mini_racer_engine::process::fork;
use mini_racer_engine::{ContextConfig, ContextHandle, Host, WaitHandle, WaitOutcome};

struct Parking;

impl Host for Parking {
    fn wait(&mut self, wait: &WaitHandle<'_>) -> WaitOutcome {
        self.wait_blocking(wait);
        WaitOutcome::Ready
    }

    fn wait_blocking(&mut self, wait: &WaitHandle<'_>) {
        while !wait.ready() {
            wait.park();
        }
    }
}

fn cache() -> &'static CodeCache {
    fork::current().code_cache()
}

fn counts() -> (u64, u64, u64) {
    (cache().stored(), cache().hits(), cache().rebuilt())
}

fn spawn() -> ContextHandle {
    ContextHandle::spawn(ContextConfig::default(), &mut Parking).expect("spawn")
}

fn eval(context: &ContextHandle, filename: &str, source: &str) -> Vec<u8> {
    context
        .eval(filename.to_owned(), source.to_owned(), &mut Parking)
        .expect("eval")
}

// A script over 128 KiB whose functions only a call to its `render` runs.
fn script(seed: u32) -> String {
    let mut text = String::new();
    for i in 0..4_000 {
        writeln!(text, "function f{seed}_{i}(a) {{ return a + {i} - {i}; }}").expect("write");
    }
    write!(
        text,
        "globalThis.render{seed} = (n) => {{ let sum = 0; for (let i = 0; i < 4000; i++) \
         sum += globalThis['f{seed}_' + i](n); return [sum, f{seed}_3999.toString()]; }};"
    )
    .expect("write");
    text
}

#[test]
fn a_cache_is_built_again_after_its_builders_next_call() {
    if std::env::var_os("MINI_RACER_RS_CODE_CACHE").is_some() {
        return;
    }
    let text = script(1);
    eval(&spawn(), "bundle.js", &text);
    let builder = spawn();
    eval(&builder, "bundle.js", &text);
    assert_eq!(counts(), (1, 0, 0));

    // Neither maintenance nor another load is a call.
    builder.heap_stats(&mut Parking).expect("heap stats");
    eval(&builder, "other.js", &script(2));
    assert_eq!(counts(), (1, 0, 0));

    let called = eval(&builder, "<eval>", "render1(3)");
    assert_eq!(counts(), (1, 0, 1));
    eval(&builder, "<eval>", "render1(4)");
    assert_eq!(counts(), (1, 0, 1));

    // A context compiling from it does not build it again.
    let reader = spawn();
    eval(&reader, "bundle.js", &text);
    assert_eq!(eval(&reader, "<eval>", "render1(3)"), called);
    assert_eq!(counts(), (1, 1, 1));

    // A collection the context asks for lets go of the script first.
    let text = script(3);
    eval(&spawn(), "bundle.js", &text);
    let collected = spawn();
    eval(&collected, "bundle.js", &text);
    collected
        .low_memory_notification(&mut Parking)
        .expect("collection");
    eval(&collected, "<eval>", "render3(3)");
    assert_eq!(counts(), (2, 1, 1));
}
