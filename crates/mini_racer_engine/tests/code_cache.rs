#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "test helpers fail the test by panicking"
)]

use std::fmt::Write;

use mini_racer_engine::process::code_cache::CodeCache;
use mini_racer_engine::process::fork;
use mini_racer_engine::{ContextConfig, ContextHandle, EngineError, Host, WaitHandle, WaitOutcome};

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

fn spawn() -> ContextHandle {
    ContextHandle::spawn(ContextConfig::default(), &mut Parking).expect("spawn")
}

fn eval(context: &ContextHandle, filename: &str, source: &str) -> Result<Vec<u8>, EngineError> {
    context.eval(filename.to_owned(), source.to_owned(), &mut Parking)
}

// A script over the size worth caching that evaluates to `result`.
fn script(seed: u32, result: u32) -> String {
    let mut text = String::new();
    for i in 0..25_000 {
        writeln!(text, "function f{seed}_{i}(a) {{ return a + {i} - {i}; }}").expect("write");
    }
    writeln!(
        text,
        "function thrower{seed}() {{ throw new Error('from the bundle'); }}"
    )
    .expect("write");
    write!(text, "f{seed}_0({result});").expect("write");
    text
}

// A script over the size worth caching, evaluated once, told apart from the
// others by its start.
fn one_off(seed: u32) -> String {
    format!("var oneOff{seed} = {seed};\n/* {} */", "x".repeat(1 << 20))
}

// Counters and entries are the process's, so the cases run in order in one
// test, as nextest's process per test and libtest's threads both allow.
#[test]
fn the_code_cache() {
    if std::env::var_os("MINI_RACER_RS_CODE_CACHE").is_some() {
        return;
    }
    let text = script(1, 1000);
    let contexts = [spawn(), spawn(), spawn()];
    let results: Vec<Vec<u8>> = contexts
        .iter()
        .map(|context| eval(context, "bundle.js", &text).expect("eval"))
        .collect();
    assert!(results.iter().all(|result| *result == results[0]));
    // Seen once, then cached by the second context before its response.
    assert_eq!(cache().stored(), 1);
    assert_eq!(cache().hits(), 1);
    let probe = "[f1_24999.toString(), (() => { try { thrower1() } catch (e) { return e.stack } })(), f1_24999(7)]";
    let from_source = eval(&contexts[0], "<eval>", probe).expect("eval");
    let from_cache = eval(&contexts[2], "<eval>", probe).expect("eval");
    assert_eq!(from_source, from_cache);

    let small = spawn();
    for _ in 0..3 {
        eval(&small, "small.js", "function g(a) { return a } g(1)").expect("eval");
    }
    assert_eq!(cache().stored(), 1);

    let throwing = format!("{}\nthrow new Error('at the end')", script(2, 1000));
    for _ in 0..3 {
        assert!(matches!(
            eval(&spawn(), "bundle.js", &throwing),
            Err(EngineError::Js(_))
        ));
    }
    assert_eq!(cache().stored(), 1);

    let awaited = format!("{}\nPromise.resolve(5)", script(3, 1000));
    let answers: Vec<Vec<u8>> = (0..3)
        .map(|_| {
            spawn()
                .eval_await("awaited.js".to_owned(), awaited.clone(), &mut Parking)
                .expect("eval_await")
        })
        .collect();
    assert!(answers.iter().all(|answer| *answer == answers[0]));
    assert_eq!(cache().stored(), 2);
    assert_eq!(cache().hits(), 2);

    // A bundle loaded again after a hundred large scripts evaluated once, as a
    // process running large per-request scripts between loads sees, is still
    // seen a second time and cached.
    let bundle = script(4, 1000);
    eval(&spawn(), "bundle.js", &bundle).expect("eval");
    let one_offs = spawn();
    for seed in 0..100 {
        eval(&one_offs, "one-off.js", &one_off(seed)).expect("eval");
    }
    eval(&spawn(), "bundle.js", &bundle).expect("eval");
    assert_eq!(cache().stored(), 3);
    eval(&spawn(), "bundle.js", &bundle).expect("eval");
    assert_eq!(cache().hits(), 3);
}

// The documented switch, in a run of this test alone with it set. Sources
// are shared under the cache's key, so it turns sharing off too.
#[test]
fn turned_off() {
    if std::env::var_os("MINI_RACER_RS_CODE_CACHE").is_some() {
        let text = script(9, 1);
        for _ in 0..3 {
            eval(&spawn(), "off.js", &text).expect("eval");
        }
        assert_eq!(cache().stored(), 0);
        assert_eq!(cache().hits(), 0);
        assert_eq!(fork::current().sources().reads(), 0);
        return;
    }
    let status = std::process::Command::new(std::env::current_exe().expect("test binary"))
        .args(["turned_off", "--exact"])
        .env("MINI_RACER_RS_CODE_CACHE", "off")
        .status()
        .expect("run the test binary");
    assert!(status.success(), "{status}");
}
