#![allow(clippy::panic, reason = "test helpers fail the test by panicking")]

use mini_racer_engine::process::platform;
use mini_racer_engine::{
    CallbackReply, ContextConfig, ContextHandle, EngineError, Host, JsErrorKind, WaitHandle,
    WaitOutcome,
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

    fn callback(&mut self, _request: &[u8]) -> CallbackReply {
        CallbackReply::Abort
    }
}

fn eval(context: &ContextHandle, source: &str) -> Result<Vec<u8>, EngineError> {
    context.eval("<eval>".to_owned(), source.to_owned(), &mut Parking)
}

fn runtime_error(result: Result<Vec<u8>, EngineError>) -> String {
    match result {
        Err(EngineError::Js(js)) if js.kind == JsErrorKind::Runtime => js.message,
        other => panic!("expected a JavaScript runtime error, got {other:?}"),
    }
}

// Flags freeze when V8 initializes, once per process, so every flag-dependent
// assertion lives in this one test.
#[test]
fn flags_apply_before_initialization() {
    platform::push_flag(b"use_strict", None).expect("flag");
    platform::push_flag(b"stack_size", Some(b"4096")).expect("flag");
    assert_eq!(platform::stack_size_kb(), Some(4096));
    let context = ContextHandle::spawn(ContextConfig::default(), &mut Parking).expect("spawn");
    assert!(runtime_error(eval(&context, "x = 1")).starts_with("ReferenceError"));
    let overflow = runtime_error(eval(&context, "function f(n) { return f(n + 1) + 1 } f(0)"));
    assert!(overflow.starts_with("RangeError"), "{overflow}");
    assert!(eval(&context, "1 + 1").is_ok());
}
