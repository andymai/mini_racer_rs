#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "test helpers fail the test by panicking"
)]

use std::process::Command;
use std::time::Duration;

use mini_racer_engine::process::platform;
use mini_racer_engine::{
    ContextConfig, ContextHandle, EngineError, Host, JsErrorKind, TerminationCause, WaitHandle,
    WaitOutcome,
};
use mini_racer_wire::{Begun, Container, Limits, Scalar, Text, Visitor, decode_value};

const LIMIT: usize = 64 << 20;
const OBJECTS: &str =
    "const a = []; for (;;) a.push({i: a.length, s: 'k' + a.length, n: [a.length, 1]});";

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

#[derive(Debug, Clone, Copy, PartialEq)]
enum Out {
    Nil,
    Bool(bool),
    Number(f64),
    Name(usize),
    Container,
}

// Reads a result's scalars, and an object's numbers by their names.
#[derive(Default)]
struct Reader {
    names: Vec<String>,
    numbers: Vec<(String, f64)>,
}

impl Visitor for Reader {
    type Value = Out;
    type Frame = ();
    type Error = String;

    fn scalar(&mut self, scalar: Scalar<'_>) -> Result<Out, String> {
        Ok(match scalar {
            Scalar::Undefined | Scalar::Null => Out::Nil,
            Scalar::Bool(b) => Out::Bool(b),
            Scalar::Int32(n) => Out::Number(n.into()),
            Scalar::Uint32(n) => Out::Number(n.into()),
            Scalar::Number(n) => Out::Number(n),
            Scalar::Text(Text::Utf8(bytes) | Text::Latin1(bytes)) => {
                self.names.push(String::from_utf8_lossy(bytes).into_owned());
                Out::Name(self.names.len() - 1)
            }
            other => return Err(format!("unexpected {other:?}")),
        })
    }

    fn begin(&mut self, _container: Container) -> Result<Begun<(), Out>, String> {
        Ok(Begun {
            frame: (),
            value: None,
        })
    }

    fn push(&mut self, (): &mut (), _value: Out) -> Result<(), String> {
        Ok(())
    }

    fn insert(&mut self, (): &mut (), key: Out, value: Out) -> Result<(), String> {
        if let (Out::Name(name), Out::Number(n)) = (key, value) {
            let name = self.names.get(name).cloned().unwrap_or_default();
            self.numbers.push((name, n));
        }
        Ok(())
    }

    fn end(&mut self, (): ()) -> Result<Out, String> {
        Ok(Out::Container)
    }

    fn register(&mut self, _id: u32, _value: Out) -> Result<(), String> {
        Ok(())
    }

    fn back_ref(&mut self, _id: u32) -> Result<Out, String> {
        Ok(Out::Container)
    }
}

fn spawn(config: ContextConfig) -> ContextHandle {
    ContextHandle::spawn(config, &mut Parking).expect("spawn")
}

fn limited(limit: usize) -> ContextHandle {
    spawn(ContextConfig {
        heap_limit: Some(limit),
        ..ContextConfig::default()
    })
}

fn decoded(bytes: &[u8]) -> (Out, Reader) {
    let mut reader = Reader::default();
    let out = decode_value(bytes, Limits::default(), &mut reader).expect("V8 output decodes");
    (out, reader)
}

fn eval(context: &ContextHandle, source: &str) -> Result<Out, EngineError> {
    let bytes = context.eval("<eval>".to_owned(), source.to_owned(), &mut Parking)?;
    Ok(decoded(&bytes).0)
}

fn text(context: &ContextHandle, source: &str) -> String {
    let bytes = context
        .eval("<eval>".to_owned(), source.to_owned(), &mut Parking)
        .expect("eval");
    let (out, reader) = decoded(&bytes);
    match out {
        Out::Name(index) => reader.names.get(index).cloned().expect("decoded"),
        other => panic!("expected a string, got {other:?}"),
    }
}

fn stat(context: &ContextHandle, name: &str) -> f64 {
    let bytes = context.heap_stats(&mut Parking).expect("heap stats");
    let (_, reader) = decoded(&bytes);
    reader
        .numbers
        .iter()
        .find_map(|(key, n)| (key == name).then_some(*n))
        .expect("a statistic of that name")
}

fn heap_size_limit(context: &ContextHandle) -> f64 {
    stat(context, "heap_size_limit")
}

#[expect(
    clippy::cast_precision_loss,
    reason = "every limit here is a whole number of MiB"
)]
const fn exactly(limit: usize) -> f64 {
    limit as f64
}

// The error and message a script past the limit ends with.
fn limit_reached(result: Result<Out, EngineError>, limit: usize) {
    match result {
        Err(EngineError::Js(js)) => {
            assert_eq!(
                js.kind,
                JsErrorKind::Terminated(TerminationCause::HeapExhausted)
            );
            assert_eq!(js.message, format!("heap limit of {limit} bytes reached"));
        }
        other => panic!("expected the heap limit, got {other:?}"),
    }
}

fn still_over(result: Result<impl std::fmt::Debug, EngineError>) {
    match result {
        Err(EngineError::Js(js)) => {
            assert_eq!(
                js.kind,
                JsErrorKind::Terminated(TerminationCause::HeapExhausted)
            );
            assert!(
                js.message.starts_with("heap still over its limit"),
                "{js:?}"
            );
        }
        other => panic!("expected a poisoned context, got {other:?}"),
    }
}

#[test]
fn a_script_past_the_limit_raises_and_the_context_carries_on() {
    let context = limited(LIMIT);
    assert_eq!(heap_size_limit(&context), exactly(LIMIT));
    limit_reached(eval(&context, &format!("(() => {{ {OBJECTS} }})()")), LIMIT);
    assert_eq!(eval(&context, "1 + 1").expect("usable"), Out::Number(2.0));
    assert_eq!(heap_size_limit(&context), exactly(LIMIT));
    let kept = "globalThis.kept = Array.from({length: 1000}, (_, i) => ({i})); kept.length";
    assert_eq!(eval(&context, kept).expect("usable"), Out::Number(1000.0));
}

#[test]
fn every_hit_on_one_context_raises() {
    let context = limited(LIMIT);
    for _ in 0..10 {
        limit_reached(eval(&context, &format!("(() => {{ {OBJECTS} }})()")), LIMIT);
        assert_eq!(heap_size_limit(&context), exactly(LIMIT));
    }
    assert_eq!(eval(&context, "1 + 1").expect("usable"), Out::Number(2.0));
}

#[test]
fn a_heap_still_over_the_limit_after_a_collection_refuses_javascript() {
    let context = limited(LIMIT);
    let retained =
        "globalThis.keep = []; for (;;) keep.push({i: keep.length, s: 'k' + keep.length});";
    limit_reached(eval(&context, retained), LIMIT);
    still_over(eval(&context, "1 + 1"));
    still_over(eval(&context, "keep = null"));
    still_over(context.perform_microtask_checkpoint(&mut Parking));
    still_over(context.pump_message_loop(&mut Parking));
    context
        .low_memory_notification(&mut Parking)
        .expect("a collection");
    still_over(eval(&context, "1 + 1"));
    assert!(stat(&context, "used_heap_size") > exactly(LIMIT / 2));
    assert!(context.heap_snapshot(&mut Parking).is_ok());
    context.begin_dispose();
    context.dispose(&mut Parking).expect("dispose");
}

// Each of these ends where V8's own code allocates, not the script's.
#[test]
fn json_and_typed_arrays_past_the_limit_raise() {
    let context = limited(LIMIT);
    let unit = r#"{"a":12345,"b":"xy","c":[1,2]}"#;
    let parse = format!(
        "(() => {{ const s = '[' + '{unit},'.repeat({}) + '{unit}]'; return JSON.parse(s).length; }})()",
        LIMIT * 2 / 5 / (unit.len() + 1)
    );
    for _ in 0..3 {
        limit_reached(eval(&context, &parse), LIMIT);
        limit_reached(
            eval(
                &context,
                "(() => { const keep = []; for (;;) keep.push(new Array(1 << 20).fill(1.5)); })()",
            ),
            LIMIT,
        );
    }
    assert_eq!(eval(&context, "1 + 1").expect("usable"), Out::Number(2.0));
}

// An ArrayBuffer's backing store lives outside the heap.
#[test]
fn array_buffers_are_not_held_to_the_limit() {
    let context = limited(LIMIT);
    let total = LIMIT * 2;
    let buffers = format!(
        "(() => {{ const keep = []; let total = 0; while (total < {total}) {{ keep.push(new ArrayBuffer(8 << 20)); total += 8 << 20; }} return keep.length; }})()"
    );
    assert_eq!(
        eval(&context, &buffers).expect("buffers"),
        Out::Number(exactly(total / (8 << 20)))
    );
}

#[test]
fn an_awaited_script_past_the_limit_raises() {
    let context = limited(LIMIT);
    let source = "(async () => { const a = []; for (let i = 0; ; i++) { if (i % 1000 === 0) await null; a.push({i, s: 'k' + i}); } })()";
    let result = context
        .eval_await("<eval>".to_owned(), source.to_owned(), &mut Parking)
        .map(|bytes| decoded(&bytes).0);
    limit_reached(result, LIMIT);
    assert_eq!(eval(&context, "1 + 1").expect("usable"), Out::Number(2.0));
}

// mini_racer runs the microtasks a terminated script left; those of a script
// that reached the limit would allocate again, so they are dropped.
#[test]
fn microtasks_a_hit_leaves_never_run() {
    let context = limited(LIMIT);
    let left = format!(
        "Promise.resolve().then(() => {{ globalThis.ran = true }}); (() => {{ {OBJECTS} }})()"
    );
    limit_reached(eval(&context, &left), LIMIT);
    assert_eq!(eval(&context, "0").expect("usable"), Out::Number(0.0));
    context
        .perform_microtask_checkpoint(&mut Parking)
        .expect("checkpoint");
    assert_eq!(text(&context, "typeof ran"), "undefined");
    let allocating =
        format!("Promise.resolve().then(() => {{ {OBJECTS} }}); (() => {{ {OBJECTS} }})()");
    limit_reached(eval(&context, &allocating), LIMIT);
    assert_eq!(eval(&context, "1 + 1").expect("usable"), Out::Number(2.0));

    let timed = spawn(ContextConfig {
        timeout: Some(Duration::from_millis(20)),
        heap_limit: Some(LIMIT),
        ..ContextConfig::default()
    });
    let looping = "Promise.resolve().then(() => { globalThis.ran = true }); while (true) {}";
    assert!(matches!(
        eval(&timed, looping),
        Err(EngineError::Js(js)) if js.kind == JsErrorKind::Terminated(TerminationCause::Timeout)
    ));
    assert_eq!(text(&timed, "typeof ran"), "boolean");
}

// Whichever of a timeout and the limit ends the script, it ends with an error.
#[test]
fn a_timeout_racing_a_hit_ends_the_script() {
    let mut causes = Vec::new();
    for millis in (10..=200).step_by(10) {
        let context = spawn(ContextConfig {
            timeout: Some(Duration::from_millis(millis)),
            heap_limit: Some(LIMIT),
            ..ContextConfig::default()
        });
        match eval(&context, &format!("(() => {{ {OBJECTS} }})()")) {
            Err(EngineError::Js(js)) => match js.kind {
                JsErrorKind::Terminated(cause) => causes.push(cause),
                other => panic!("{other:?}"),
            },
            other => panic!("{other:?}"),
        }
        assert_eq!(eval(&context, "1 + 1").expect("usable"), Out::Number(2.0));
    }
    assert!(
        causes.contains(&TerminationCause::HeapExhausted),
        "{causes:?}"
    );
}

#[test]
fn a_context_without_a_limit_keeps_v8s() {
    let context = spawn(ContextConfig::default());
    assert!(heap_size_limit(&context) > exactly(platform::MAX_HEAP_LIMIT / 4));
}

const FLAG: &str = "MINI_RACER_RS_TEST_HEAP_FLAG";

// Run in a process of its own per flag: the flag, then whether a heap limit
// is refused, then the heap V8 sizes for a context with one.
fn sized_under(flag: &str) -> (bool, f64) {
    let output = Command::new(std::env::current_exe().expect("test binary"))
        .args(["flag_in_its_own_process", "--exact", "--nocapture"])
        .env(FLAG, flag)
        .output()
        .expect("run the test binary");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{flag}: {stdout}");
    let line = stdout
        .lines()
        .find_map(|line| line.strip_prefix("sized "))
        .expect("a result");
    let (refused, limit) = line.split_once(' ').expect("two fields");
    (refused == "refused", limit.parse().expect("a number"))
}

#[test]
fn flag_in_its_own_process() {
    let Ok(flag) = std::env::var(FLAG) else {
        return;
    };
    let (name, value) = match flag.split_once('=') {
        Some((name, value)) => (name, Some(value.as_bytes())),
        None => (flag.as_str(), None),
    };
    platform::push_flag(name.as_bytes(), value).expect("flag");
    let refused = platform::set_heap_limit(Some(LIMIT)).is_err();
    let limit = heap_size_limit(&limited(LIMIT));
    println!(
        "sized {} {limit}",
        if refused { "refused" } else { "taken" }
    );
}

// V8 15.2's `Heap::ConfigureHeap` sizes the heap from these flags in place
// of an isolate's own limits, in every spelling V8 reads; the others leave
// the limit alone.
#[test]
fn flags_that_size_the_heap_override_the_limit_and_are_refused() {
    for flag in [
        "max_old_space_size=128",
        "--max-old-space-size=128",
        "no-max-old-space-size=128",
        "max_old_space_size=8",
        "max_heap_size=256",
        "max-heap-size=32",
        "max_semi_space_size=16",
        "nomax_semi_space_size=1",
        "stack_size=900 --max_heap_size=128",
    ] {
        let (refused, limit) = sized_under(flag);
        assert!(refused, "{flag}");
        assert_ne!(limit, exactly(LIMIT), "{flag}");
    }
    for flag in [
        "max_old_space_size=0",
        "max_young_generation_size=64",
        "initial_heap_size=48",
        "initial_old_space_size=48",
        "min_semi_space_size=8",
        "max_shared_heap_size=8",
        "use_strict",
    ] {
        let (refused, limit) = sized_under(flag);
        assert!(!refused, "{flag}");
        assert_eq!(limit, exactly(LIMIT), "{flag}");
    }
    // These imply a semi-space size and are taken: 1 MiB leaves the heap
    // under the limit, and the 4 MiB of a flag for testing V8's collector
    // puts it up to 6 MiB over a limit under 128 MiB.
    for (flag, semi_spaces) in [
        ("optimize_for_size", -1),
        ("stress_compaction", -1),
        ("predictable_gc_schedule", 2),
    ] {
        let (refused, limit) = sized_under(flag);
        assert!(!refused, "{flag}");
        let sized = LIMIT.checked_add_signed(semi_spaces * (3 << 20));
        assert_eq!(limit, exactly(sized.expect("a size")), "{flag}");
    }
}
