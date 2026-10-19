//! A list of shared sources every one of which a live context still reads,
//! in a process of its own, as the list is the process's.

#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "test helpers fail the test by panicking"
)]

use std::fmt::Write;

use mini_racer_engine::process::fork;
use mini_racer_engine::process::sources::Sources;
use mini_racer_engine::{ContextConfig, ContextHandle, Host, JsText, WaitHandle, WaitOutcome};

// What the list holds while every source in it is read.
const KEPT: u64 = 8;

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

fn loaded(text: &JsText) -> ContextHandle {
    let context = ContextHandle::spawn(ContextConfig::default(), &mut Parking).expect("spawn");
    context
        .eval(JsText::from("bundle.js"), text.clone(), &mut Parking)
        .expect("eval");
    context
}

fn probe(context: &ContextHandle, seed: u32) -> String {
    let source = format!(
        "[f{seed}_24999.toString(), (() => {{ try {{ thrower{seed}() }} catch (e) {{ return e.stack }} }})()]"
    );
    format!(
        "{:?}",
        context.eval(JsText::from("<eval>"), JsText::from(source), &mut Parking)
    )
}

// Over a megabyte, and two bytes a unit in V8.
fn script(seed: u32) -> JsText {
    let mut text = String::from("// \u{1F600}\n");
    for i in 0..25_000 {
        writeln!(text, "function f{seed}_{i}(a) {{ return a + {i} - {i}; }}").expect("write");
    }
    write!(
        text,
        "function thrower{seed}() {{ throw new Error('from the bundle'); }}\nf{seed}_0(1);"
    )
    .expect("write");
    JsText::from(text)
}

#[test]
fn a_full_list_leaves_another_script_to_copies_of_its_own_unconverted() {
    if std::env::var_os("MINI_RACER_RS_CODE_CACHE").is_some() {
        return;
    }
    let readers: Vec<ContextHandle> = (0..u32::try_from(KEPT).unwrap_or(0))
        .map(|seed| loaded(&script(seed)))
        .collect();
    assert_eq!((sources().made(), sources().reads()), (KEPT, KEPT));
    let converted = sources().conversions();

    let extra = script(100);
    let own: Vec<ContextHandle> = (0..3).map(|_| loaded(&extra)).collect();
    assert_eq!(sources().reads(), KEPT);
    assert_eq!(sources().conversions(), converted);
    let expected = probe(&own[0], 100);
    assert!(expected.starts_with("Ok("), "{expected}");
    for context in &own[1..] {
        assert_eq!(probe(context, 100), expected);
    }

    // One kept source no longer read makes room for it.
    let reader = &readers[0];
    reader.begin_dispose();
    reader.dispose(&mut Parking).expect("dispose");
    let shared = loaded(&extra);
    assert_eq!(
        (sources().made(), sources().reads(), sources().conversions()),
        (KEPT + 1, KEPT + 1, converted + 1)
    );
    assert_eq!(probe(&shared, 100), expected);
}
