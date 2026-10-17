#![allow(clippy::panic, reason = "test helpers fail the test by panicking")]

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use mini_racer_engine::{
    ContextConfig, ContextHandle, EngineError, Host, JsErrorKind, JsText, TerminationCause,
    WaitHandle, WaitOutcome,
};
use mini_racer_wire::{Begun, Container, Limits, Scalar, Visitor, decode_value};

#[derive(Debug, Clone, Copy, PartialEq)]
enum Out {
    Nil,
    Bool(bool),
    Int(i64),
    Num(f64),
    Text,
}

struct Shape;

impl Visitor for Shape {
    type Value = Out;
    type Frame = ();
    type Error = String;

    fn scalar(&mut self, scalar: Scalar<'_>) -> Result<Out, String> {
        Ok(match scalar {
            Scalar::Undefined | Scalar::Null => Out::Nil,
            Scalar::Bool(b) => Out::Bool(b),
            Scalar::Int32(n) => Out::Int(n.into()),
            Scalar::Uint32(n) => Out::Int(n.into()),
            Scalar::Number(n) => Out::Num(n),
            Scalar::Text(_) => Out::Text,
            other => return Err(format!("unexpected {other:?}")),
        })
    }

    fn begin(&mut self, container: Container) -> Result<Begun<(), Out>, String> {
        Err(format!("unexpected {container:?}"))
    }

    fn push(&mut self, (): &mut (), _value: Out) -> Result<(), String> {
        Ok(())
    }

    fn insert(&mut self, (): &mut (), _key: Out, _value: Out) -> Result<(), String> {
        Ok(())
    }

    fn end(&mut self, (): ()) -> Result<Out, String> {
        Err("unexpected container".to_owned())
    }

    fn register(&mut self, _id: u32, _value: Out) -> Result<(), String> {
        Ok(())
    }

    fn back_ref(&mut self, id: u32) -> Result<Out, String> {
        Err(format!("unexpected reference {id}"))
    }
}

/// Parks until the runner answers; never cancels.
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

/// Cancels once `interrupted` is set, as a Ruby interrupt would.
struct Interruptible<'a> {
    interrupted: &'a AtomicBool,
}

impl Host for Interruptible<'_> {
    fn wait(&mut self, wait: &WaitHandle<'_>) -> WaitOutcome {
        while !wait.spin(Duration::from_millis(1)) {
            if self.interrupted.load(Ordering::SeqCst) {
                return WaitOutcome::Cancel;
            }
        }
        WaitOutcome::Ready
    }

    fn wait_blocking(&mut self, wait: &WaitHandle<'_>) {
        Parking.wait_blocking(wait);
    }
}

fn eval(context: &ContextHandle, source: &str) -> Result<Out, EngineError> {
    let bytes = context.eval("<eval>".to_owned(), source.to_owned(), &mut Parking)?;
    Ok(decode_value(&bytes, Limits::default(), &mut Shape).expect("V8 output decodes"))
}

fn spawn() -> ContextHandle {
    ContextHandle::spawn(ContextConfig::default(), &mut Parking).expect("spawn")
}

fn terminated_by(result: Result<Out, EngineError>) -> TerminationCause {
    match result {
        Err(EngineError::Js(js)) => match js.kind {
            JsErrorKind::Terminated(cause) => {
                assert_eq!(js.message, "terminated");
                cause
            }
            other => panic!("expected termination, got {other:?}"),
        },
        other => panic!("expected termination, got {other:?}"),
    }
}

#[test]
fn evaluates_primitives() {
    let context = spawn();
    assert_eq!(eval(&context, "1 + 1").ok(), Some(Out::Int(2)));
    assert_eq!(eval(&context, "0.5").ok(), Some(Out::Num(0.5)));
    assert_eq!(eval(&context, "null").ok(), Some(Out::Nil));
    assert_eq!(eval(&context, "undefined").ok(), Some(Out::Nil));
    assert_eq!(eval(&context, "1 < 2").ok(), Some(Out::Bool(true)));
    assert_eq!(eval(&context, "'hi'").ok(), Some(Out::Text));
    assert!(mini_racer_engine::process::v8_version().starts_with("15.2."));
}

#[test]
fn reports_errors_in_mini_racers_formats() {
    let context = spawn();
    let cases = [
        (
            "1 +",
            JsErrorKind::Parse,
            "Uncaught SyntaxError: Unexpected end of input at <eval>:1:3",
        ),
        (
            "\n\n  foo bar",
            JsErrorKind::Parse,
            "Uncaught SyntaxError: Unexpected identifier 'bar' at <eval>:3:6",
        ),
        (
            "throw new Error('boom')",
            JsErrorKind::Runtime,
            "Error: boom\n    at <eval>:1:7",
        ),
        ("throw 'str'", JsErrorKind::Runtime, "str"),
        ("throw {a: 1}", JsErrorKind::Runtime, "[object Object]"),
        ("throw null", JsErrorKind::Runtime, "null"),
    ];
    for (source, kind, message) in cases {
        match eval(&context, source) {
            Err(EngineError::Js(js)) => assert_eq!((js.kind, js.message.as_str()), (kind, message)),
            other => panic!("{source}: expected an error, got {other:?}"),
        }
    }
    match context.eval("f.js".to_owned(), "1 +".to_owned(), &mut Parking) {
        Err(EngineError::Js(js)) => assert!(js.message.ends_with(" at f.js:1:3"), "{}", js.message),
        other => panic!("expected a parse error, got {other:?}"),
    }
    assert_eq!(eval(&context, "2 * 21").ok(), Some(Out::Int(42)));
}

#[test]
fn contexts_are_isolated() {
    let a = spawn();
    let b = spawn();
    assert_eq!(eval(&a, "var shared = 7; shared").ok(), Some(Out::Int(7)));
    assert_eq!(
        eval(&b, "typeof shared === 'undefined'").ok(),
        Some(Out::Bool(true))
    );
}

#[test]
fn stop_terminates_a_running_script() {
    let context = spawn();
    std::thread::scope(|scope| {
        scope.spawn(|| {
            std::thread::sleep(Duration::from_millis(50));
            context.stop().expect("live context");
        });
        assert_eq!(
            terminated_by(eval(&context, "while (true) {}")),
            TerminationCause::Stop
        );
    });
    assert_eq!(eval(&context, "6 * 7").ok(), Some(Out::Int(42)));
}

#[test]
fn a_stop_while_idle_terminates_the_next_script_only() {
    let context = spawn();
    context.stop().expect("live context");
    assert_eq!(terminated_by(eval(&context, "1")), TerminationCause::Stop);
    assert_eq!(eval(&context, "2").ok(), Some(Out::Int(2)));
}

#[test]
fn a_timeout_terminates_only_its_own_script() {
    let context = ContextHandle::spawn(
        ContextConfig {
            timeout: Some(Duration::from_millis(50)),
            ..ContextConfig::default()
        },
        &mut Parking,
    )
    .expect("spawn");
    let started = Instant::now();
    assert_eq!(
        terminated_by(eval(&context, "while (true) {}")),
        TerminationCause::Timeout
    );
    assert!(started.elapsed() < Duration::from_secs(10));
    for n in 0..200 {
        assert_eq!(eval(&context, &n.to_string()).ok(), Some(Out::Int(n)));
    }
}

#[test]
// Creating an isolate and its two contexts takes well over a millisecond, so a
// timed creation would fail here.
fn creating_the_isolate_does_not_count_against_a_timeout() {
    for _ in 0..10 {
        ContextHandle::spawn(
            ContextConfig {
                timeout: Some(Duration::from_millis(1)),
                ..ContextConfig::default()
            },
            &mut Parking,
        )
        .expect("spawn");
    }
}

#[test]
fn a_timeout_reaches_a_getter_run_while_serializing() {
    let context = ContextHandle::spawn(
        ContextConfig {
            timeout: Some(Duration::from_millis(50)),
            ..ContextConfig::default()
        },
        &mut Parking,
    )
    .expect("spawn");
    assert_eq!(
        terminated_by(eval(&context, "var a = {get a() { while (true); }}; a")),
        TerminationCause::Timeout
    );
    assert_eq!(eval(&context, "1").ok(), Some(Out::Int(1)));
}

#[test]
fn a_cancelled_wait_terminates_the_script_and_leaves_nothing_behind() {
    let context = spawn();
    let interrupted = AtomicBool::new(false);
    std::thread::scope(|scope| {
        let result = scope.spawn(|| {
            let mut host = Interruptible {
                interrupted: &interrupted,
            };
            context.eval("<eval>".to_owned(), "while (true) {}".to_owned(), &mut host)
        });
        std::thread::sleep(Duration::from_millis(50));
        interrupted.store(true, Ordering::SeqCst);
        let result = result.join().expect("waiter thread");
        assert_eq!(
            terminated_by(result.map(|_| Out::Nil)),
            TerminationCause::Cancelled
        );
    });
    assert_eq!(eval(&context, "6 * 7").ok(), Some(Out::Int(42)));
}

#[test]
fn dispose_is_final_and_repeatable() {
    let context = spawn();
    assert_eq!(eval(&context, "1").ok(), Some(Out::Int(1)));
    context.begin_dispose();
    context.dispose(&mut Parking).expect("dispose");
    assert!(context.is_disposed());
    assert!(matches!(eval(&context, "1"), Err(EngineError::Disposed)));
    assert!(matches!(context.stop(), Err(EngineError::Disposed)));
    context
        .dispose(&mut Parking)
        .expect("a second dispose is a no-op");
}

#[test]
fn dispose_terminates_a_script_running_on_another_thread() {
    let context = spawn();
    std::thread::scope(|scope| {
        let running = scope.spawn(|| eval(&context, "while (true) {}"));
        std::thread::sleep(Duration::from_millis(50));
        context.begin_dispose();
        assert_eq!(
            terminated_by(running.join().expect("eval thread")),
            TerminationCause::Stop
        );
    });
    context.dispose(&mut Parking).expect("dispose");
    assert!(matches!(eval(&context, "1"), Err(EngineError::Disposed)));
}

#[test]
fn dropping_a_handle_shuts_its_runner_down_without_blocking() {
    for _ in 0..20 {
        let context = spawn();
        assert_eq!(eval(&context, "1").ok(), Some(Out::Int(1)));
        drop(context);
    }
}

fn eval_await(context: &ContextHandle, source: &str) -> Result<Out, EngineError> {
    let bytes = context.eval_await("<eval>", source, &mut Parking)?;
    Ok(decode_value(&bytes, Limits::default(), &mut Shape).expect("V8 output decodes"))
}

const NEVER: &str = "new Promise(() => {})";
// A promise V8 settles from a delayed task of its own, 30 ms on.
const DELAYED: &str = "Atomics.waitAsync(new Int32Array(new SharedArrayBuffer(4)), 0, 0, 30).value";

#[test]
fn an_await_runs_microtasks_until_the_promise_settles() {
    let context = spawn();
    let chain = "(async () => { for (let i = 0; i < 100; i++) await null; return 7 })()";
    assert_eq!(eval_await(&context, chain).expect("settles"), Out::Int(7));
    assert_eq!(
        eval_await(&context, "6 * 7").expect("no promise"),
        Out::Int(42)
    );
}

#[test]
fn an_awaited_rejection_is_a_runtime_error_with_its_stack() {
    let context = spawn();
    match eval_await(&context, "Promise.reject(new Error('nope'))") {
        Err(EngineError::Js(js)) => {
            assert_eq!(js.kind, JsErrorKind::Runtime);
            assert!(js.message.starts_with("Error: nope\n"), "{}", js.message);
        }
        other => panic!("expected a runtime error, got {other:?}"),
    }
    match eval_await(&context, "Promise.reject('plain')") {
        Err(EngineError::Js(js)) => assert_eq!(js.message, "plain"),
        other => panic!("expected a runtime error, got {other:?}"),
    }
}

#[test]
fn an_await_sleeps_until_a_delayed_task_is_due() {
    let context = spawn();
    let started = Instant::now();
    let settled = eval_await(&context, &format!("{DELAYED}.then(v => v === 'timed-out')"));
    assert_eq!(settled.expect("settles"), Out::Bool(true));
    assert!(started.elapsed() >= Duration::from_millis(25));
}

#[test]
fn a_stop_ends_an_await_that_never_settles() {
    let context = spawn();
    std::thread::scope(|scope| {
        scope.spawn(|| {
            std::thread::sleep(Duration::from_millis(50));
            context.stop().expect("live context");
        });
        assert_eq!(
            terminated_by(eval_await(&context, NEVER)),
            TerminationCause::Stop
        );
    });
    assert_eq!(eval(&context, "1 + 1").expect("usable"), Out::Int(2));
}

#[test]
fn a_timeout_ends_an_await_and_only_that_one() {
    let context = ContextHandle::spawn(
        ContextConfig {
            timeout: Some(Duration::from_millis(50)),
            ..ContextConfig::default()
        },
        &mut Parking,
    )
    .expect("spawn");
    assert_eq!(
        terminated_by(eval_await(&context, NEVER)),
        TerminationCause::Timeout
    );
    assert_eq!(eval(&context, "1 + 1").expect("usable"), Out::Int(2));
}

#[test]
fn a_cancelled_wait_ends_an_await() {
    let context = spawn();
    let interrupted = AtomicBool::new(false);
    let result = std::thread::scope(|scope| {
        scope.spawn(|| {
            std::thread::sleep(Duration::from_millis(50));
            interrupted.store(true, Ordering::SeqCst);
        });
        context.eval_await(
            "<eval>",
            NEVER,
            &mut Interruptible {
                interrupted: &interrupted,
            },
        )
    });
    let result = result.map(|_| Out::Nil);
    assert_eq!(terminated_by(result), TerminationCause::Cancelled);
    assert_eq!(eval(&context, "1 + 1").expect("usable"), Out::Int(2));
}

#[test]
fn a_pump_runs_one_due_task_then_the_microtasks_it_queued() {
    let context = spawn();
    assert!(!context.pump_message_loop(&mut Parking).expect("pump"));
    eval(
        &context,
        &format!("globalThis.done = false; {DELAYED}.then(() => {{ done = true }}); 0"),
    )
    .expect("eval");
    assert!(!context.pump_message_loop(&mut Parking).expect("pump"));
    std::thread::sleep(Duration::from_millis(50));
    assert!(context.pump_message_loop(&mut Parking).expect("pump"));
    assert_eq!(eval(&context, "done").expect("eval"), Out::Bool(true));
    assert!(!context.pump_message_loop(&mut Parking).expect("pump"));
}

#[test]
fn a_pump_consumes_a_stop_and_a_checkpoint_leaves_it() {
    let context = spawn();
    context.stop().expect("live context");
    context
        .perform_microtask_checkpoint(&mut Parking)
        .expect("checkpoint");
    assert_eq!(terminated_by(eval(&context, "1")), TerminationCause::Stop);
    context.stop().expect("live context");
    assert!(!context.pump_message_loop(&mut Parking).expect("pump"));
    assert_eq!(eval(&context, "1 + 1").expect("usable"), Out::Int(2));
}

#[test]
fn an_await_after_dispose_is_refused() {
    let context = spawn();
    context.begin_dispose();
    context.dispose(&mut Parking).expect("dispose");
    assert!(matches!(
        eval_await(&context, "1"),
        Err(EngineError::Disposed)
    ));
    assert!(matches!(
        context.pump_message_loop(&mut Parking),
        Err(EngineError::Disposed)
    ));
}

// mini_racer's last termination check comes before the result is serialized,
// so a timeout that fires while a large string is copied out leaves the
// result, and the next script.
#[test]
fn a_timeout_while_the_result_serializes_leaves_the_result() {
    let length = 64 << 20;
    let blob = ContextHandle::create_snapshot(
        JsText::from(format!("var value = 'x'.repeat({length})")),
        None,
        &mut Parking,
    )
    .expect("snapshot");
    let context = ContextHandle::spawn(
        ContextConfig {
            timeout: Some(Duration::from_millis(5)),
            snapshot: Some(blob),
        },
        &mut Parking,
    )
    .expect("spawn");
    let started = Instant::now();
    let bytes = context
        .eval("<eval>", "value", &mut Parking)
        .expect("the result");
    assert!(bytes.len() > length);
    assert!(
        started.elapsed() > Duration::from_millis(5),
        "serializing outlasted the timeout"
    );
    assert_eq!(eval(&context, "1 + 1").expect("usable"), Out::Int(2));
}
