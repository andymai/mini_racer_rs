#![allow(clippy::panic, reason = "test helpers fail the test by panicking")]

use std::time::{Duration, Instant};

use mini_racer_engine::{
    CallbackReply, ContextConfig, ContextHandle, EngineError, Host, JsErrorKind, JsText,
    TerminationCause, WaitHandle, WaitOutcome,
};
use mini_racer_wire::{Begun, Container, Limits, Scalar, Visitor, Writer, decode_value};

// Integers, and arrays of them flattened in order. A callback's request is a
// sparse array, as mini_racer builds it with a length up front.
#[derive(Default)]
struct Ints(Vec<i64>);

impl Visitor for Ints {
    type Value = i64;
    type Frame = ();
    type Error = String;

    fn scalar(&mut self, scalar: Scalar<'_>) -> Result<i64, String> {
        match scalar {
            Scalar::Int32(n) => Ok(n.into()),
            Scalar::Uint32(n) => Ok(n.into()),
            other => Err(format!("unexpected {other:?}")),
        }
    }

    fn begin(&mut self, container: Container) -> Result<Begun<(), i64>, String> {
        match container {
            Container::DenseArray { .. } | Container::SparseArray { .. } => Ok(Begun {
                frame: (),
                value: None,
            }),
            other => Err(format!("unexpected {other:?}")),
        }
    }

    fn push(&mut self, (): &mut (), value: i64) -> Result<(), String> {
        self.0.push(value);
        Ok(())
    }

    fn insert(&mut self, (): &mut (), _index: i64, value: i64) -> Result<(), String> {
        self.0.push(value);
        Ok(())
    }

    fn end(&mut self, (): ()) -> Result<i64, String> {
        Ok(-1)
    }

    fn register(&mut self, _id: u32, _value: i64) -> Result<(), String> {
        Ok(())
    }

    fn back_ref(&mut self, id: u32) -> Result<i64, String> {
        Err(format!("unexpected reference {id}"))
    }
}

fn ints(bytes: &[u8]) -> Vec<i64> {
    let mut ints = Ints::default();
    let root = decode_value(bytes, Limits::default(), &mut ints).expect("decodes");
    if root != -1 {
        ints.0.push(root);
    }
    ints.0
}

fn integer(value: i64) -> Vec<u8> {
    let mut out = Vec::new();
    Writer::new(&mut out).integer(value);
    out
}

fn name_and_id(name: &str, id: i64) -> Vec<u8> {
    let mut out = Vec::new();
    let mut writer = Writer::new(&mut out);
    writer.begin_array(2);
    writer.utf8(name.as_bytes());
    writer.integer(id);
    writer.end_array(2);
    out
}

fn parked(wait: &WaitHandle<'_>) {
    while !wait.ready() {
        wait.park();
    }
}

struct Answering<F>(F);

impl<F: FnMut(&[i64], i64) -> CallbackReply> Host for Answering<F> {
    fn wait(&mut self, wait: &WaitHandle<'_>) -> WaitOutcome {
        parked(wait);
        WaitOutcome::Ready
    }

    fn wait_blocking(&mut self, wait: &WaitHandle<'_>) {
        parked(wait);
    }

    fn callback(&mut self, request: &[u8]) -> CallbackReply {
        let mut values = ints(request);
        let id = values.pop().expect("the id comes last");
        (self.0)(&values, id)
    }
}

fn adding() -> Answering<impl FnMut(&[i64], i64) -> CallbackReply> {
    Answering(|args: &[i64], _id| CallbackReply::Value(integer(args.iter().sum())))
}

fn spawn() -> ContextHandle {
    ContextHandle::spawn(ContextConfig::default(), &mut adding()).expect("spawn")
}

fn eval(
    context: &ContextHandle,
    source: &str,
    host: &mut impl Host,
) -> Result<Vec<i64>, EngineError> {
    context
        .eval("<eval>".to_owned(), source.to_owned(), host)
        .map(|bytes| ints(&bytes))
}

#[test]
fn a_callback_gets_its_arguments_and_id_and_returns_its_reply() {
    let context = spawn();
    context
        .attach(name_and_id("math.add", 7), &mut adding())
        .expect("attach");
    let mut seen = Vec::new();
    let mut host = Answering(|args: &[i64], id| {
        seen.push((args.to_vec(), id));
        CallbackReply::Value(integer(args.iter().sum()))
    });

    assert_eq!(
        eval(&context, "math.add(2, 3) * 10", &mut host).expect("eval"),
        [50]
    );
    assert_eq!(seen, [(vec![2, 3], 7)]);
}

#[test]
fn a_callback_dispatches_nested_on_its_own_thread_only() {
    let context = spawn();
    context
        .attach(name_and_id("inner", 0), &mut adding())
        .expect("attach");
    let mut busy = None;
    let mut host = Answering(|_args: &[i64], _id| {
        std::thread::scope(|scope| {
            busy = Some(scope.spawn(|| eval(&context, "1", &mut adding())).join());
        });
        match context.eval("<eval>".to_owned(), "6 * 7".to_owned(), &mut adding()) {
            Ok(bytes) => CallbackReply::Value(bytes),
            Err(error) => panic!("nested eval: {error}"),
        }
    });

    assert_eq!(
        eval(&context, "inner() + 1", &mut host).expect("eval"),
        [43]
    );
    assert!(matches!(busy, Some(Ok(Err(EngineError::Busy)))));
    assert!(!context.in_callback());
}

#[test]
fn a_raised_callback_throws_and_reports_when_it_escapes() {
    let context = spawn();
    context
        .attach(name_and_id("fails", 0), &mut adding())
        .expect("attach");
    let mut host = Answering(|_args: &[i64], _id| CallbackReply::Raised(JsText::from("nope")));

    let escaped = eval(&context, "fails()", &mut host);
    assert!(matches!(escaped, Err(EngineError::Js(js)) if js.kind == JsErrorKind::HostException));
    let caught = context.eval(
        "<eval>".to_owned(),
        "try { fails() } catch (e) { e.message.length }".to_owned(),
        &mut host,
    );
    assert_eq!(ints(&caught.expect("caught")), [4]);
}

#[test]
fn an_aborted_callback_terminates_its_dispatch_and_leaves_the_context_usable() {
    let context = spawn();
    context
        .attach(name_and_id("aborts", 0), &mut adding())
        .expect("attach");
    let mut host = Answering(|_args: &[i64], _id| CallbackReply::Abort);

    let aborted = eval(&context, "aborts(); while (true) {}", &mut host);
    assert!(matches!(
        aborted,
        Err(EngineError::Js(js)) if js.kind == JsErrorKind::Terminated(TerminationCause::Cancelled)
    ));
    assert_eq!(eval(&context, "1 + 2", &mut adding()).expect("eval"), [3]);
}

#[test]
fn attach_replaces_segments_that_are_not_objects() {
    let context = spawn();
    eval(&context, "globalThis.minion = 2; 0", &mut adding()).expect("eval");
    context
        .attach(name_and_id("minion.kevin.speak", 0), &mut adding())
        .expect("attach");

    assert_eq!(
        eval(&context, "minion.kevin.speak(40, 2)", &mut adding()).expect("eval"),
        [42]
    );
}

#[test]
fn an_await_nested_in_a_callback_is_refused_and_the_outer_one_settles() {
    let context = spawn();
    context
        .attach(name_and_id("inner", 0), &mut adding())
        .expect("attach");
    let mut refusal = None;
    let mut host = Answering(|_args: &[i64], _id| {
        refusal = Some(context.eval_await("<eval>", "Promise.resolve(1)", &mut adding()));
        CallbackReply::Value(integer(41))
    });

    let settled = context
        .eval_await(
            "<eval>",
            "(async () => { await null; return inner() + 1 })()",
            &mut host,
        )
        .map(|bytes| ints(&bytes));
    assert_eq!(settled.expect("settles"), [42]);
    match refusal {
        Some(Err(EngineError::Js(js))) => {
            assert_eq!(js.kind, JsErrorKind::Runtime);
            assert!(
                js.message
                    .starts_with("Error: nested call_await/eval_await is not supported"),
                "{}",
                js.message
            );
        }
        other => panic!("expected the refusal, got {other:?}"),
    }
}

// Gives a dispatch up after two seconds, as a Ruby timeout would, so a hang
// fails the test instead of stalling it.
struct Deadline(Instant);

impl Host for Deadline {
    fn wait(&mut self, wait: &WaitHandle<'_>) -> WaitOutcome {
        while !wait.spin(Duration::from_millis(1)) {
            if self.0.elapsed() > Duration::from_secs(2) {
                return WaitOutcome::Cancel;
            }
        }
        WaitOutcome::Ready
    }

    fn wait_blocking(&mut self, wait: &WaitHandle<'_>) {
        parked(wait);
    }
}

// Nothing times a pump, so mini_racer times a script a callback runs inside
// one on its own: the script is stopped, and the pump's task carries on.
#[test]
fn a_script_nested_in_a_pump_is_timed_on_its_own() {
    let context = ContextHandle::spawn(
        ContextConfig {
            timeout: Some(Duration::from_millis(50)),
            ..ContextConfig::default()
        },
        &mut adding(),
    )
    .expect("spawn");
    context
        .attach(name_and_id("spin", 0), &mut adding())
        .expect("attach");
    eval(
        &context,
        "globalThis.got = 0; \
         Atomics.waitAsync(new Int32Array(new SharedArrayBuffer(4)), 0, 0, 5).value \
           .then(() => { got = spin() }); 0",
        &mut adding(),
    )
    .expect("eval");
    std::thread::sleep(Duration::from_millis(30));
    let mut nested = None;
    let mut host = Answering(|_args: &[i64], _id| {
        nested = Some(context.eval("<eval>", "while (true) {}", &mut Deadline(Instant::now())));
        CallbackReply::Value(integer(7))
    });

    assert!(context.pump_message_loop(&mut host).expect("pump"));
    assert!(
        matches!(
            nested,
            Some(Err(EngineError::Js(ref js))) if js.kind == JsErrorKind::Terminated(TerminationCause::Timeout)
        ),
        "{nested:?}"
    );
    assert_eq!(eval(&context, "got", &mut adding()).expect("eval"), [7]);
}

// mini_racer throws the refusal, then reports it as terminated when a stop is
// pending, keeping the refusal's text.
#[test]
fn a_nested_await_under_a_pending_stop_reports_the_refusal_as_terminated() {
    let context = spawn();
    context
        .attach(name_and_id("stops", 0), &mut adding())
        .expect("attach");
    let mut refusal = None;
    let mut host = Answering(|_args: &[i64], _id| {
        context.stop().expect("live context");
        refusal = Some(context.eval_await("<eval>", "1", &mut adding()));
        CallbackReply::Value(integer(0))
    });

    let outer = eval(&context, "stops()", &mut host);
    assert!(matches!(
        outer,
        Err(EngineError::Js(js)) if js.kind == JsErrorKind::Terminated(TerminationCause::Stop)
    ));
    match refusal {
        Some(Err(EngineError::Js(js))) => {
            assert_eq!(js.kind, JsErrorKind::Terminated(TerminationCause::Stop));
            assert!(
                js.message
                    .starts_with("Error: nested call_await/eval_await is not supported"),
                "{}",
                js.message
            );
        }
        other => panic!("expected the refusal, got {other:?}"),
    }
    assert_eq!(eval(&context, "1 + 1", &mut adding()).expect("eval"), [2]);
}

// mini_racer's last termination check for a script nested in a callback also
// comes before its result is serialized: the outer script's timeout firing
// while a large nested result is copied out leaves that result. The callback
// starts about 5 ms before the timeout; copying 128 MiB took about 20 ms on
// the fastest CI runner measured.
#[test]
fn a_timeout_while_a_nested_result_serializes_leaves_the_result() {
    let length = 128 << 20;
    let blob = ContextHandle::create_snapshot(
        JsText::from(format!("var value = 'x'.repeat({length})")),
        None,
        &mut adding(),
    )
    .expect("snapshot");
    let context = ContextHandle::spawn(
        ContextConfig {
            timeout: Some(Duration::from_millis(50)),
            snapshot: Some(blob),
            ..ContextConfig::default()
        },
        &mut adding(),
    )
    .expect("spawn");
    context
        .attach(name_and_id("grab", 0), &mut adding())
        .expect("attach");
    let mut nested = None;
    let mut host = Answering(|_args: &[i64], _id| {
        nested = Some(
            context
                .eval("<eval>", "value", &mut adding())
                .map(|bytes| bytes.len()),
        );
        CallbackReply::Value(integer(0))
    });

    let outer = eval(
        &context,
        "const end = Date.now() + 45; while (Date.now() < end) {} grab(); 1",
        &mut host,
    );
    assert!(
        matches!(nested, Some(Ok(bytes)) if bytes > length),
        "{nested:?}"
    );
    assert!(
        matches!(
            outer,
            Err(EngineError::Js(ref js)) if js.kind == JsErrorKind::Terminated(TerminationCause::Timeout)
        ),
        "{outer:?}"
    );
}

// A dispose cancels mini_racer's wait for a nested script, so one landing
// while the nested result serializes still fails it. Copying 128 MiB took
// about 20 ms on the fastest CI runner measured, past the 5 ms the dispose
// waits.
#[test]
fn a_dispose_while_a_nested_result_serializes_fails_it() {
    let length = 128 << 20;
    let blob = ContextHandle::create_snapshot(
        JsText::from(format!("var value = 'x'.repeat({length})")),
        None,
        &mut adding(),
    )
    .expect("snapshot");
    let context = ContextHandle::spawn(
        ContextConfig {
            snapshot: Some(blob),
            ..ContextConfig::default()
        },
        &mut adding(),
    )
    .expect("spawn");
    context
        .attach(name_and_id("grab", 0), &mut adding())
        .expect("attach");
    let mut nested = None;
    std::thread::scope(|scope| {
        let mut host = Answering(|_args: &[i64], _id| {
            scope.spawn(|| {
                std::thread::sleep(Duration::from_millis(5));
                context.begin_dispose();
            });
            nested = Some(
                context
                    .eval("<eval>", "value", &mut adding())
                    .map(|bytes| bytes.len()),
            );
            CallbackReply::Value(integer(0))
        });
        assert!(eval(&context, "grab(); 1", &mut host).is_err());
    });
    assert!(matches!(nested, Some(Err(_))), "{nested:?}");
    context.dispose(&mut adding()).expect("dispose");
}

// mini_racer throws a raising callback's exception even with a stop pending:
// the exception escaping the script takes precedence, and the stop takes over
// once a script catches it.
#[test]
fn a_callback_raising_under_a_pending_stop_still_throws() {
    let context = spawn();
    context
        .attach(name_and_id("fails", 0), &mut adding())
        .expect("attach");
    let mut host = Answering(|_args: &[i64], _id| {
        context.stop().expect("live context");
        CallbackReply::Raised(JsText::from("x"))
    });

    let escaped = eval(&context, "fails(); 5", &mut host);
    assert!(
        matches!(escaped, Err(EngineError::Js(ref js)) if js.kind == JsErrorKind::HostException),
        "{escaped:?}"
    );
    let caught = eval(&context, "try { fails() } catch (e) { 7 }", &mut host);
    assert!(
        matches!(caught, Err(EngineError::Js(ref js)) if js.kind == JsErrorKind::Terminated(TerminationCause::Stop)),
        "{caught:?}"
    );
    assert_eq!(eval(&context, "1 + 1", &mut adding()).expect("eval"), [2]);
}

// mini_racer lifts a nested script's termination before its callback goes
// on, and only asks for it again, so the exception the callback then raises
// is still thrown: escaping, it is raised; caught, the timeout takes over.
#[test]
fn a_callback_raising_after_its_nested_script_was_terminated_still_throws() {
    let context = ContextHandle::spawn(
        ContextConfig {
            timeout: Some(Duration::from_millis(50)),
            ..ContextConfig::default()
        },
        &mut adding(),
    )
    .expect("spawn");
    context
        .attach(name_and_id("work", 0), &mut adding())
        .expect("attach");
    let mut nested = Vec::new();
    let mut host = Answering(|_args: &[i64], _id| {
        nested.push(
            context
                .eval("<eval>", "for (;;) {}", &mut adding())
                .map(drop),
        );
        CallbackReply::Raised(JsText::from("wrapped"))
    });

    let escaped = eval(&context, "work(); 5", &mut host);
    let caught = eval(&context, "try { work() } catch (e) { 7 }", &mut host);
    assert!(
        matches!(escaped, Err(EngineError::Js(ref js)) if js.kind == JsErrorKind::HostException),
        "{escaped:?}"
    );
    assert!(
        matches!(caught, Err(EngineError::Js(ref js)) if js.kind == JsErrorKind::Terminated(TerminationCause::Timeout)),
        "{caught:?}"
    );
    assert!(nested.iter().all(|nested| matches!(
        nested,
        Err(EngineError::Js(js)) if js.kind == JsErrorKind::Terminated(TerminationCause::Timeout)
    )));
}

// mini_racer's attach never asks whether a termination is pending, so under a
// stop it fails as an internal error whose text is V8's termination exception;
// the next script, outermost or nested, still reports the stop.
#[test]
fn attach_under_a_pending_stop_is_an_internal_error_and_leaves_the_stop() {
    let context = spawn();
    context.stop().expect("live context");
    match context.attach(name_and_id("f", 0), &mut adding()) {
        Err(EngineError::Js(js)) => {
            assert_eq!(js.kind, JsErrorKind::Internal);
            assert_eq!(js.message, "null");
        }
        other => panic!("expected an internal error, got {other:?}"),
    }
    assert!(matches!(
        eval(&context, "1", &mut adding()),
        Err(EngineError::Js(js)) if js.kind == JsErrorKind::Terminated(TerminationCause::Stop)
    ));
    assert!(matches!(
        eval(&context, "f()", &mut adding()),
        Err(EngineError::Js(js)) if js.kind == JsErrorKind::Runtime
    ));

    context
        .attach(name_and_id("nest", 0), &mut adding())
        .expect("attach");
    let mut nested = None;
    let mut host = Answering(|_args: &[i64], _id| {
        context.stop().expect("live context");
        nested = Some(context.attach(name_and_id("g", 1), &mut adding()));
        CallbackReply::Value(integer(0))
    });
    let outer = eval(&context, "nest(); 1", &mut host);
    assert!(
        matches!(
            nested,
            Some(Err(EngineError::Js(ref js))) if js.kind == JsErrorKind::Internal && js.message == "null"
        ),
        "{nested:?}"
    );
    assert!(matches!(
        outer,
        Err(EngineError::Js(js)) if js.kind == JsErrorKind::Terminated(TerminationCause::Stop)
    ));
    assert_eq!(eval(&context, "1 + 1", &mut adding()).expect("eval"), [2]);
}

// mini_racer's stop is a one-shot interrupt plus a flag the next script
// reports when it ends. A failed attach uses up the interrupt, so a second
// attach succeeds and the next script runs before it reports the stop.
#[test]
fn a_failed_attach_spends_the_stop_and_the_next_script_runs() {
    let context = spawn();
    context.stop().expect("live context");
    assert!(context.attach(name_and_id("f", 0), &mut adding()).is_err());
    context
        .attach(name_and_id("g", 1), &mut adding())
        .expect("the interrupt is spent");
    assert!(matches!(
        eval(&context, "globalThis.x = 1; 1", &mut adding()),
        Err(EngineError::Js(js)) if js.kind == JsErrorKind::Terminated(TerminationCause::Stop)
    ));
    assert_eq!(
        eval(&context, "x + g(2)", &mut adding()).expect("eval"),
        [3]
    );
}

// As mini_racer reads a callback's reply: a stop requested during the
// callback ends the script as it returns, before the next statement.
#[test]
fn a_stop_from_a_callback_ends_the_script_as_the_callback_returns() {
    let context = spawn();
    context
        .attach(name_and_id("stops", 0), &mut adding())
        .expect("attach");
    let mut host = Answering(|_args: &[i64], _id| {
        context.stop().expect("live context");
        CallbackReply::Value(integer(5))
    });
    assert!(matches!(
        eval(&context, "globalThis.z = 0; stops(); globalThis.z = 1; 1", &mut host),
        Err(EngineError::Js(js)) if js.kind == JsErrorKind::Terminated(TerminationCause::Stop)
    ));
    assert_eq!(eval(&context, "z", &mut adding()).expect("eval"), [0]);
}

// A nested attach that fails under the stop uses up its interrupt, and the
// script around it runs on and reports the stop when it ends.
#[test]
fn a_nested_attach_spending_the_stop_lets_the_outer_script_run_on() {
    let context = spawn();
    context
        .attach(name_and_id("nest", 0), &mut adding())
        .expect("attach");
    let mut nested = None;
    let mut host = Answering(|_args: &[i64], _id| {
        context.stop().expect("live context");
        nested = Some(context.attach(name_and_id("g", 1), &mut adding()));
        CallbackReply::Value(integer(5))
    });
    assert!(matches!(
        eval(&context, "globalThis.z = 0; nest(); globalThis.z = 1; 1", &mut host),
        Err(EngineError::Js(js)) if js.kind == JsErrorKind::Terminated(TerminationCause::Stop)
    ));
    assert!(
        matches!(nested, Some(Err(EngineError::Js(ref js))) if js.kind == JsErrorKind::Internal)
    );
    assert_eq!(eval(&context, "z", &mut adding()).expect("eval"), [1]);
}

fn stopper_and_raiser(context: &ContextHandle) {
    context
        .attach(name_and_id("stopper", 0), &mut adding())
        .expect("attach");
    context
        .attach(name_and_id("raiser", 1), &mut adding())
        .expect("attach");
}

fn stopping(context: &ContextHandle) -> Answering<impl FnMut(&[i64], i64) -> CallbackReply + '_> {
    Answering(move |_args: &[i64], id| {
        context.stop().expect("live context");
        if id == 1 {
            CallbackReply::Raised(JsText::from("boom"))
        } else {
            CallbackReply::Value(integer(1))
        }
    })
}

fn stopped(result: Result<Vec<i64>, EngineError>) -> bool {
    matches!(result, Err(EngineError::Js(js)) if js.kind == JsErrorKind::Terminated(TerminationCause::Stop))
}

// A terminated script's leftover microtasks run as mini_racer's reply runs
// them. A stop V8 acts on there is carried, spent: the next script runs, then
// reports it.
#[test]
fn a_stop_acted_on_in_leftover_microtasks_carries_to_the_next_script() {
    let context = spawn();
    stopper_and_raiser(&context);
    let source = "Promise.resolve().then(() => { stopper() }); stopper(); 0";
    assert!(stopped(eval(&context, source, &mut stopping(&context))));
    assert!(stopped(eval(
        &context,
        "globalThis.x = 1; 1",
        &mut adding()
    )));
    assert_eq!(eval(&context, "x", &mut adding()).expect("eval"), [1]);
}

// One still pending when they finish fails mini_racer's reply, and its retry
// reports the stop with the terminated script.
#[test]
fn a_stop_pending_after_leftover_microtasks_ends_with_the_script() {
    let context = spawn();
    stopper_and_raiser(&context);
    let source = "Promise.resolve().then(() => { raiser() }); stopper(); 0";
    assert!(stopped(eval(&context, source, &mut stopping(&context))));
    assert_eq!(eval(&context, "1 + 1", &mut adding()).expect("eval"), [2]);
}

// A deadline that has not fired yet bounds them, as mini_racer's watchdog
// does, rather than a fresh one.
#[test]
fn leftover_microtasks_of_a_stopped_script_keep_its_deadline() {
    let context = ContextHandle::spawn(
        ContextConfig {
            timeout: Some(Duration::from_millis(300)),
            ..ContextConfig::default()
        },
        &mut adding(),
    )
    .expect("spawn");
    stopper_and_raiser(&context);
    let busy = |ms: u32| format!("{{ const t = Date.now(); while (Date.now() - t < {ms}) {{}} }}");
    let source = format!(
        "Promise.resolve().then(() => {{ {}; globalThis.after = 1 }}); {}; stopper(); 0",
        busy(250),
        busy(200)
    );
    assert!(stopped(eval(&context, &source, &mut stopping(&context))));
    assert_eq!(
        eval(
            &context,
            "globalThis.after === undefined ? 0 : 1",
            &mut adding()
        )
        .expect("eval"),
        [0]
    );
}
