#![allow(clippy::panic, reason = "test helpers fail the test by panicking")]

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
