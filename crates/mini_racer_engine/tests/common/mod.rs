#![allow(clippy::panic, reason = "test helpers fail the test by panicking")]

use std::time::Duration;

use mini_racer_engine::{
    CallbackReply, ContextConfig, ContextHandle, EngineError, Host, JsErrorKind, WaitHandle,
    WaitOutcome,
};

pub struct Parking;

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

fn terminated(result: Result<Vec<u8>, EngineError>) -> bool {
    matches!(result, Err(EngineError::Js(js)) if matches!(js.kind, JsErrorKind::Terminated(_)))
}

pub fn churn() {
    std::thread::scope(|scope| {
        for _ in 0..4 {
            scope.spawn(|| {
                for round in 0..25 {
                    let context = ContextHandle::spawn(ContextConfig::default(), &mut Parking)
                        .expect("spawn");
                    assert!(
                        context
                            .eval(
                                "<eval>".to_owned(),
                                "[1, 'two', 3.5]".to_owned(),
                                &mut Parking
                            )
                            .is_ok()
                    );
                    if round % 3 == 0 {
                        context.stop().expect("live context");
                        assert!(terminated(context.eval(
                            "<eval>".to_owned(),
                            "1".to_owned(),
                            &mut Parking
                        )));
                    }
                    context.begin_dispose();
                    context.dispose(&mut Parking).expect("dispose");
                    if round % 5 == 0 {
                        let timed = ContextHandle::spawn(
                            ContextConfig {
                                timeout: Some(Duration::from_millis(20)),
                            },
                            &mut Parking,
                        )
                        .expect("spawn");
                        assert!(terminated(timed.eval(
                            "<eval>".to_owned(),
                            "while (true) {}".to_owned(),
                            &mut Parking
                        )));
                        timed.begin_dispose();
                        timed.dispose(&mut Parking).expect("dispose");
                    }
                }
            });
        }
    });
}
