//! A process keeps at most 64 code caches, dropping the least recently used;
//! in a file of its own, as the cache is the process's.

#![allow(
    clippy::expect_used,
    reason = "test helpers fail the test by panicking"
)]

use std::fmt::Write;

use mini_racer_engine::process::code_cache::CodeCache;
use mini_racer_engine::process::fork;
use mini_racer_engine::{ContextConfig, ContextHandle, Host, WaitHandle, WaitOutcome};

// What the cache holds before it drops an entry.
const KEPT: usize = 64;

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

fn counts() -> (u64, u64) {
    (cache().stored(), cache().hits())
}

fn load(text: &str) {
    let context = ContextHandle::spawn(ContextConfig::default(), &mut Parking).expect("spawn");
    context
        .eval("bundle.js".to_owned(), text.to_owned(), &mut Parking)
        .expect("eval");
    context.begin_dispose();
    context.dispose(&mut Parking).expect("dispose");
}

// A script over 128 KiB, the size worth caching, and under a megabyte.
fn script(seed: usize) -> String {
    let mut text = String::new();
    for i in 0..4_000 {
        writeln!(text, "function f{seed}_{i}(a) {{ return a + {i} - {i}; }}").expect("write");
    }
    write!(text, "f{seed}_0(1);").expect("write");
    text
}

#[test]
fn a_cache_past_the_count_drops_the_least_recently_used() {
    if std::env::var_os("MINI_RACER_RS_CODE_CACHE").is_some() {
        return;
    }
    let texts: Vec<String> = (0..=KEPT).map(script).collect();
    let stored = u64::try_from(KEPT).expect("a count");
    // Each cached at its second sighting.
    for text in &texts[..KEPT] {
        load(text);
        load(text);
    }
    assert_eq!(counts(), (stored, 0));
    // Found, the first becomes the most recently used, so the one past the
    // count drops the second, which is then made again.
    load(&texts[0]);
    assert_eq!(counts(), (stored, 1));
    load(&texts[KEPT]);
    load(&texts[KEPT]);
    assert_eq!(counts(), (stored + 1, 1));
    load(&texts[1]);
    assert_eq!(counts(), (stored + 2, 1));
    load(&texts[0]);
    load(&texts[KEPT]);
    assert_eq!(counts(), (stored + 2, 3));
}
