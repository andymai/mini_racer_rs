#![allow(clippy::panic, reason = "test helpers fail the test by panicking")]

use std::time::Duration;

use mini_racer_engine::{ContextHandle, EngineError, JsErrorKind, WaitOutcome};
use mini_racer_wire::{Scalar, Visitor, decode_value};

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
    type Error = std::convert::Infallible;

    fn scalar(&mut self, scalar: Scalar<'_>) -> Result<Out, Self::Error> {
        Ok(match scalar {
            Scalar::Undefined | Scalar::Null => Out::Nil,
            Scalar::Bool(b) => Out::Bool(b),
            Scalar::Int32(n) => Out::Int(n.into()),
            Scalar::Uint32(n) => Out::Int(n.into()),
            Scalar::Number(n) => Out::Num(n),
            Scalar::Utf8(_) | Scalar::Latin1(_) | Scalar::Utf16Le(_) => Out::Text,
        })
    }
}

fn eval(context: &ContextHandle, source: &str) -> Result<Out, EngineError> {
    let pending = context.eval(source.to_owned())?;
    let WaitOutcome::Done(result) = pending.wait() else {
        panic!("nothing interrupts this wait");
    };
    Ok(decode_value(&result?, &mut Shape).expect("V8 output decodes"))
}

#[test]
fn evaluates_primitives() {
    let context = ContextHandle::spawn().expect("spawn");
    assert_eq!(eval(&context, "1 + 1").ok(), Some(Out::Int(2)));
    assert_eq!(eval(&context, "0.5").ok(), Some(Out::Num(0.5)));
    assert_eq!(eval(&context, "null").ok(), Some(Out::Nil));
    assert_eq!(eval(&context, "undefined").ok(), Some(Out::Nil));
    assert_eq!(eval(&context, "1 < 2").ok(), Some(Out::Bool(true)));
    assert_eq!(eval(&context, "'hi'").ok(), Some(Out::Text));
    assert!(mini_racer_engine::process::v8_version().starts_with("15.2."));
}

#[test]
fn reports_parse_and_runtime_errors() {
    let context = ContextHandle::spawn().expect("spawn");
    match eval(&context, "1 +") {
        Err(EngineError::Js(js)) => assert_eq!(js.kind, JsErrorKind::Parse),
        other => panic!("expected a parse error, got {other:?}"),
    }
    match eval(&context, "throw new Error('boom')") {
        Err(EngineError::Js(js)) => {
            assert_eq!(js.kind, JsErrorKind::Runtime);
            assert_eq!(js.message, "Error: boom");
        }
        other => panic!("expected a runtime error, got {other:?}"),
    }
    assert_eq!(eval(&context, "2 * 21").ok(), Some(Out::Int(42)));
}

#[test]
fn contexts_are_isolated() {
    let a = ContextHandle::spawn().expect("spawn");
    let b = ContextHandle::spawn().expect("spawn");
    assert_eq!(eval(&a, "var shared = 7; shared").ok(), Some(Out::Int(7)));
    assert_eq!(
        eval(&b, "typeof shared === 'undefined'").ok(),
        Some(Out::Bool(true))
    );
}

#[test]
fn terminates_a_running_script() {
    let context = ContextHandle::spawn().expect("spawn");
    let pending = context.eval("while (true) {}".to_owned()).expect("post");
    std::thread::scope(|scope| {
        scope.spawn(|| {
            std::thread::sleep(Duration::from_millis(50));
            pending.interrupt();
        });
        assert!(matches!(pending.wait(), WaitOutcome::Interrupted));
    });
    context.terminate(&pending);
    match pending.wait_for(Duration::from_secs(10)) {
        Some(Err(EngineError::Js(js))) => assert_eq!(js.kind, JsErrorKind::Terminated),
        other => panic!("expected termination, got {other:?}"),
    }
    assert_eq!(eval(&context, "6 * 7").ok(), Some(Out::Int(42)));
}
