#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "test helpers fail the test by panicking"
)]

use std::fmt::Write;

use mini_racer_engine::process::fork;
use mini_racer_engine::process::sources::Sources;
use mini_racer_engine::{
    ContextConfig, ContextHandle, EngineError, Host, JsText, SnapshotBlob, WaitHandle, WaitOutcome,
};

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

fn spawn_with(config: ContextConfig) -> ContextHandle {
    ContextHandle::spawn(config, &mut Parking).expect("spawn")
}

fn spawn() -> ContextHandle {
    spawn_with(ContextConfig::default())
}

// mini_racer counts the source against `max_memory:`, so a context with one
// keeps its own copy, which the shared one must read the same as.
fn own_copy() -> ContextHandle {
    spawn_with(ContextConfig {
        max_memory: Some(1 << 30),
        ..ContextConfig::default()
    })
}

fn eval(context: &ContextHandle, source: &JsText) -> Result<Vec<u8>, EngineError> {
    context.eval(JsText::from("bundle.js"), source.clone(), &mut Parking)
}

fn probe(context: &ContextHandle, source: &str) -> String {
    format!(
        "{:?}",
        context.eval(JsText::from("<eval>"), JsText::from(source), &mut Parking)
    )
}

// A script of over a megabyte, two bytes a unit in V8 for its astral
// characters, with a BOM, a NUL, a line separator and Latin-1 in a function
// the probes read back, and a function that throws.
fn script(seed: u32) -> String {
    let mut text = String::from("\u{FEFF}// \u{1F600} caf\u{E9}\n");
    for i in 0..25_000 {
        writeln!(text, "function f{seed}_{i}(a) {{ return a + {i} - {i}; }}").expect("write");
    }
    write!(
        text,
        "function odd{seed}() {{ /* \u{1F600}\0\u{2028}caf\u{E9} */ return '\u{1F601}'; }}\n\
         function thrower{seed}() {{ throw new Error('from the bundle \u{1F602}'); }}\n\
         f{seed}_0(1);"
    )
    .expect("write");
    text
}

fn probes(seed: u32) -> Vec<String> {
    let reflect = "Error.prepareStackTrace = (_, sites) => sites.map((site) => [site.getFileName(), \
                   site.getLineNumber(), site.getColumnNumber(), site.getPosition(), \
                   site.getScriptHash(), site.getFunctionName(), site.isToplevel()])";
    vec![
        format!("[odd{seed}.toString(), odd{seed}.toString().length, odd{seed}()]"),
        format!("f{seed}_24999.toString()"),
        format!("(() => {{ try {{ thrower{seed}() }} catch (e) {{ return e.stack }} }})()"),
        format!(
            "(() => {{ {reflect}; try {{ thrower{seed}() }} catch (e) {{ return e.stack }} \
             finally {{ delete Error.prepareStackTrace }} }})()"
        ),
        format!("thrower{seed}()"),
    ]
}

// Counters and kept sources are the process's, so the cases run in order in
// one test, as nextest's process per test and libtest's threads both allow.
#[test]
fn shared_sources() {
    if std::env::var_os("MINI_RACER_RS_CODE_CACHE").is_some() {
        return;
    }
    let text = JsText::from(script(1));
    let contexts = [spawn(), spawn(), spawn()];
    let own = own_copy();
    let results: Vec<String> = contexts
        .iter()
        .chain([&own])
        .map(|context| format!("{:?}", eval(context, &text)))
        .collect();
    assert!(results.iter().all(|result| *result == results[0]));
    // Kept by the first context, at its first sighting, and read by all three.
    assert_eq!(sources().made(), 1);
    assert_eq!(sources().reads(), 3);
    assert_eq!(sources().reading(), 3);
    for probe_source in probes(1) {
        let from_own = probe(&own, &probe_source);
        assert!(!from_own.contains("Err(Internal"), "{from_own}");
        for context in &contexts {
            assert_eq!(probe(context, &probe_source), from_own, "{probe_source}");
        }
    }

    // Evaluated again, one of the context's two strings is garbage, which the
    // next collection releases.
    eval(&contexts[1], &text).expect("eval");
    assert_eq!(sources().reads(), 4);
    contexts[1]
        .low_memory_notification(&mut Parking)
        .expect("collection");
    assert_eq!(sources().reading(), 3);
    contexts[2].begin_dispose();
    contexts[2].dispose(&mut Parking).expect("dispose");
    assert_eq!(sources().reading(), 2);

    let later = spawn();
    eval(&later, &text).expect("eval");
    assert_eq!((sources().made(), sources().reads()), (1, 5));

    // A context from a snapshot keys the script apart, as the code cache does.
    let built = ContextHandle::create_snapshot("var shared = 1".into(), None, &mut Parking)
        .expect("snapshot");
    let blob = SnapshotBlob::new(&built).expect("a blob V8 wrote");
    for _ in 0..2 {
        let from_blob = spawn_with(ContextConfig {
            snapshot: Some(blob.clone()),
            ..ContextConfig::default()
        });
        eval(&from_blob, &text).expect("eval");
    }
    assert_eq!((sources().made(), sources().reads()), (2, 7));

    // A parse error and a runtime error read the same from the shared copy as
    // from a copy of their own.
    for broken in [
        format!("{}\nfunction (", script(2)),
        format!("{}\nthrower3()", script(3)),
    ] {
        let broken = JsText::from(broken);
        let outcomes: Vec<String> = [spawn(), spawn(), spawn(), own_copy()]
            .iter()
            .map(|context| format!("{:?}", eval(context, &broken)))
            .collect();
        assert!(outcomes[0].starts_with("Err(Js("), "{}", outcomes[0]);
        assert!(outcomes.iter().all(|outcome| *outcome == outcomes[0]));
    }
    assert_eq!((sources().made(), sources().reads()), (4, 13));

    // Invalid UTF-8 is left to V8's decoder, so each context keeps its own.
    let mut invalid = script(6).into_bytes();
    invalid.extend_from_slice(b"\n// \xFF\xC3(\nf6_1(1);");
    let invalid = JsText::Utf8(invalid);
    let outcomes: Vec<String> = (0..3)
        .map(|_| format!("{:?}", eval(&spawn(), &invalid)))
        .collect();
    assert!(outcomes[0].starts_with("Ok("), "{}", outcomes[0]);
    assert!(outcomes.iter().all(|outcome| *outcome == outcomes[0]));
    assert_eq!((sources().made(), sources().reads()), (4, 13));

    // Across encodings: UTF-16 with a lone surrogate in a string, and
    // Latin-1.
    let mut units: Vec<u16> = script(4).encode_utf16().collect();
    units.extend("\nvar lone4 = '".encode_utf16());
    units.push(0xD800);
    units.extend("';".encode_utf16());
    let latin1: Vec<u8> = script(5)
        .chars()
        .map(|unit| u8::try_from(unit).unwrap_or(b' '))
        .collect();
    for (seed, text) in [(4, JsText::Utf16(units)), (5, JsText::Latin1(latin1))] {
        let contexts = [spawn(), spawn()];
        let own = own_copy();
        for context in contexts.iter().chain([&own]) {
            eval(context, &text).expect("eval");
        }
        let mut checks = probes(seed);
        checks.push("[typeof lone4, typeof lone4 === 'string' && lone4.charCodeAt(0)]".to_owned());
        for probe_source in checks {
            let from_own = probe(&own, &probe_source);
            for context in &contexts {
                assert_eq!(probe(context, &probe_source), from_own, "{probe_source}");
            }
        }
    }
    assert_eq!((sources().made(), sources().reads()), (6, 17));
}
