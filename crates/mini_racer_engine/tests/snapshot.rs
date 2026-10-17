#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "test helpers fail the test by panicking"
)]

use mini_racer_engine::{ContextConfig, ContextHandle, EngineError, Host, WaitHandle, WaitOutcome};
use mini_racer_wire::{Begun, Container, Limits, Scalar, Visitor, decode_value};

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

struct Int;

impl Visitor for Int {
    type Value = i64;
    type Frame = ();
    type Error = String;

    fn scalar(&mut self, scalar: Scalar<'_>) -> Result<i64, String> {
        match scalar {
            Scalar::Int32(n) => Ok(n.into()),
            other => Err(format!("unexpected {other:?}")),
        }
    }

    fn begin(&mut self, container: Container) -> Result<Begun<(), i64>, String> {
        Err(format!("unexpected {container:?}"))
    }

    fn push(&mut self, (): &mut (), _value: i64) -> Result<(), String> {
        Ok(())
    }

    fn insert(&mut self, (): &mut (), _key: i64, _value: i64) -> Result<(), String> {
        Ok(())
    }

    fn end(&mut self, (): ()) -> Result<i64, String> {
        Err("unexpected container".to_owned())
    }

    fn register(&mut self, _id: u32, _value: i64) -> Result<(), String> {
        Ok(())
    }

    fn back_ref(&mut self, id: u32) -> Result<i64, String> {
        Err(format!("unexpected reference {id}"))
    }
}

fn snapshot(code: &str, base: Option<Vec<u8>>) -> Result<Vec<u8>, EngineError> {
    ContextHandle::create_snapshot(code.into(), base, &mut Parking)
}

fn eval_in(blob: &[u8], source: &str) -> i64 {
    let config = ContextConfig {
        snapshot: Some(blob.to_vec()),
        ..ContextConfig::default()
    };
    let context = ContextHandle::spawn(config, &mut Parking).expect("spawn");
    let bytes = context
        .eval("<eval>".to_owned(), source.to_owned(), &mut Parking)
        .expect("eval");
    decode_value(&bytes, Limits::default(), &mut Int).expect("an integer")
}

#[test]
fn a_context_starts_from_its_snapshot() {
    let blob =
        snapshot("var base = 40; function add(x) { return base + x }", None).expect("snapshot");
    assert_eq!(eval_in(&blob, "add(2)"), 42);
}

#[test]
fn a_warmup_keeps_the_snapshot_and_none_of_its_own_state() {
    let blob = snapshot("var a = 5; function f(x) { return x * 2 }", None).expect("snapshot");
    let warmed = snapshot("f(21); var a = 7; var b = 1;", Some(blob)).expect("warmup");
    assert_eq!(eval_in(&warmed, "f(a)"), 10);
    assert_eq!(eval_in(&warmed, "typeof b === 'undefined' ? 1 : 0"), 1);
}

#[test]
fn a_failing_script_reports_mini_racers_message_and_place() {
    let Err(EngineError::Snapshot(message)) = snapshot("var foo = bar;", None) else {
        panic!("expected a snapshot error");
    };
    assert_eq!(
        message,
        "Uncaught ReferenceError: bar is not defined\n<snapshot>:1:10"
    );
    let Err(EngineError::Snapshot(message)) = snapshot("(", None) else {
        panic!("expected a snapshot error");
    };
    assert_eq!(
        message,
        "Uncaught SyntaxError: Unexpected end of input\n<snapshot>:1:1"
    );
}
