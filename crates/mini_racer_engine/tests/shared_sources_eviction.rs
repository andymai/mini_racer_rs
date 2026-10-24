//! A full list of shared sources makes room for a new one by dropping a copy
//! no string reads, which two contexts read and so stays until then; in a
//! process of its own, as the list is the process's.

#![allow(
    clippy::expect_used,
    reason = "test helpers fail the test by panicking"
)]

use std::fmt::Write;

use mini_racer_engine::process::fork;
use mini_racer_engine::process::sources::Sources;
use mini_racer_engine::{ContextConfig, ContextHandle, Host, JsText, WaitHandle, WaitOutcome};

// What the list holds before it drops a copy.
const KEPT: usize = 8;

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

fn sources() -> &'static Sources {
    fork::current().sources()
}

fn counts() -> (u64, u64) {
    (sources().made(), sources().reads())
}

fn loaded(text: &JsText) -> ContextHandle {
    let context = ContextHandle::spawn(ContextConfig::default(), &mut Parking).expect("spawn");
    context
        .eval(JsText::from("bundle.js"), text.clone(), &mut Parking)
        .expect("eval");
    context
}

fn dispose(contexts: Option<[ContextHandle; 2]>) {
    for context in contexts.into_iter().flatten() {
        context.begin_dispose();
        context.dispose(&mut Parking).expect("dispose");
    }
}

// Over a megabyte.
fn script(seed: usize) -> JsText {
    let mut text = String::new();
    for i in 0..25_000 {
        writeln!(text, "function f{seed}_{i}(a) {{ return a + {i} - {i}; }}").expect("write");
    }
    write!(text, "f{seed}_0(1);").expect("write");
    JsText::from(text)
}

#[test]
fn a_ninth_source_drops_a_copy_no_string_reads() {
    if std::env::var_os("MINI_RACER_RS_CODE_CACHE").is_some() {
        return;
    }
    let texts: Vec<JsText> = (0..=KEPT).map(script).collect();
    let mut readers: Vec<Option<[ContextHandle; 2]>> = texts[..KEPT]
        .iter()
        .map(|text| Some([loaded(text), loaded(text)]))
        .collect();
    let made = u64::try_from(KEPT).expect("a count");
    assert_eq!(counts(), (made, 2 * made));
    // Unread, and not the least recently used.
    dispose(readers[3].take());
    assert_eq!(sources().reading(), 2 * KEPT - 2);
    let _ninth = loaded(&texts[KEPT]);
    assert_eq!(counts(), (made + 1, 2 * made + 1));
    // Room for the fourth again, made again as it was dropped.
    dispose(readers[0].take());
    let _fourth = loaded(&texts[3]);
    assert_eq!(counts(), (made + 2, 2 * made + 2));
}
