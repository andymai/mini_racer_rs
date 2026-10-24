//! A first sighting of 16 MiB or more, in a context sharing its source, is
//! cached at once when its run took at least as long as its compile. Each case
//! sets `MINI_RACER_RS_CODE_CACHE_FIRST` in place of the timing and runs in a
//! process of its own, as the setting and the cache are the process's.

#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "test helpers fail the test by panicking"
)]

use std::fmt::Write;

use mini_racer_engine::process::code_cache::CodeCache;
use mini_racer_engine::process::fork;
use mini_racer_engine::{ContextConfig, ContextHandle, EngineError, Host, WaitHandle, WaitOutcome};

const DECIDED: &str = "MINI_RACER_RS_CODE_CACHE_FIRST";
const FIRST_SIGHTING: usize = 16 << 20;

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

fn spawn_with(config: ContextConfig) -> ContextHandle {
    ContextHandle::spawn(config, &mut Parking).expect("spawn")
}

fn spawn() -> ContextHandle {
    spawn_with(ContextConfig::default())
}

fn eval(context: &ContextHandle, filename: &str, source: &str) -> Result<Vec<u8>, EngineError> {
    context.eval(filename.to_owned(), source.to_owned(), &mut Parking)
}

// A script of exactly `bytes`: functions, then a comment.
fn bundle(seed: u32, bytes: usize) -> String {
    let mut text = String::new();
    for i in 0..2_000 {
        writeln!(text, "function f{seed}_{i}(a) {{ return a + {i} - {i}; }}").expect("write");
    }
    writeln!(
        text,
        "function thrower{seed}() {{ throw new Error('from the bundle'); }}"
    )
    .expect("write");
    write!(text, "f{seed}_1(41);\n/*").expect("write");
    let fill = bytes - text.len() - "*/".len();
    text.push_str(&"x".repeat(fill));
    text.push_str("*/");
    text
}

// Runs `test` again in a process of its own with the timing decided.
fn alone(test: &str, decided: &str) -> bool {
    if std::env::var_os(DECIDED).is_some() {
        return false;
    }
    let status = std::process::Command::new(std::env::current_exe().expect("test binary"))
        .args([test, "--exact"])
        .env(DECIDED, decided)
        .status()
        .expect("run the test binary");
    assert!(status.success(), "{status}");
    true
}

#[test]
fn a_first_sighting_worth_a_cache_is_cached_at_once() {
    if alone("a_first_sighting_worth_a_cache_is_cached_at_once", "always")
        || std::env::var_os("MINI_RACER_RS_CODE_CACHE").is_some()
    {
        return;
    }
    let text = bundle(1, FIRST_SIGHTING);
    let first = spawn();
    let from_source = eval(&first, "bundle.js", &text).expect("eval");
    assert_eq!(counts(), (1, 0));
    let second = spawn();
    assert_eq!(
        eval(&second, "bundle.js", &text).expect("eval"),
        from_source
    );
    assert_eq!(counts(), (1, 1));
    let probe = "[f1_1999.toString(), (() => { try { thrower1() } catch (e) { return e.stack } })(), f1_1999(7)]";
    assert_eq!(
        eval(&first, "<eval>", probe).expect("eval"),
        eval(&second, "<eval>", probe).expect("eval")
    );

    // Under 16 MiB, at its second sighting.
    let smaller = bundle(2, FIRST_SIGHTING - 1);
    for (stored, hits) in [(1, 1), (2, 1), (2, 2)] {
        eval(&spawn(), "bundle.js", &smaller).expect("eval");
        assert_eq!(counts(), (stored, hits));
    }

    // A context keeping its own copy of the source has no key for it yet.
    let own = bundle(3, FIRST_SIGHTING);
    let own_copy = spawn_with(ContextConfig {
        max_memory: Some(1 << 30),
        ..ContextConfig::default()
    });
    eval(&own_copy, "bundle.js", &own).expect("eval");
    assert_eq!(counts(), (2, 2));

    // Nor is a first sighting that throws cached.
    let throwing = format!(
        "{}\nthrow new Error('at the end')",
        bundle(4, FIRST_SIGHTING)
    );
    assert!(matches!(
        eval(&spawn(), "bundle.js", &throwing),
        Err(EngineError::Js(_))
    ));
    assert_eq!(counts(), (2, 2));
}

// With 64 entries, each read, a first sighting's cache would drop one a
// context has read, so it is refused; seen again, the script is cached as
// any other, dropping the least recently used.
#[test]
fn a_first_sighting_that_would_drop_a_read_entry_is_refused() {
    if alone(
        "a_first_sighting_that_would_drop_a_read_entry_is_refused",
        "always",
    ) || std::env::var_os("MINI_RACER_RS_CODE_CACHE").is_some()
    {
        return;
    }
    // Seen, cached and read by a context each, every script new to each.
    let (seer, builder, reader) = (spawn(), spawn(), spawn());
    for seed in 0..64 {
        let text = bundle(100 + seed, 128 << 10);
        for context in [&seer, &builder, &reader] {
            eval(context, "small.js", &text).expect("eval");
        }
    }
    assert_eq!(counts(), (64, 64));
    assert_eq!((cache().entries(), cache().refusals()), (64, 0));
    let text = bundle(6, FIRST_SIGHTING);
    eval(&spawn(), "bundle.js", &text).expect("eval");
    assert_eq!(counts(), (64, 64));
    assert_eq!((cache().entries(), cache().refusals()), (64, 1));
    eval(&spawn(), "bundle.js", &text).expect("eval");
    assert_eq!(counts(), (65, 64));
    assert_eq!((cache().entries(), cache().refusals()), (64, 1));
}

#[test]
fn a_first_sighting_not_worth_a_cache_is_cached_at_its_second() {
    if alone(
        "a_first_sighting_not_worth_a_cache_is_cached_at_its_second",
        "never",
    ) || std::env::var_os("MINI_RACER_RS_CODE_CACHE").is_some()
    {
        return;
    }
    let text = bundle(5, FIRST_SIGHTING);
    for (stored, hits) in [(0, 0), (1, 0), (1, 1)] {
        eval(&spawn(), "bundle.js", &text).expect("eval");
        assert_eq!(counts(), (stored, hits));
    }
}
