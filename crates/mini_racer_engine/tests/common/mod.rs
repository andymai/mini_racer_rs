#![allow(clippy::panic, reason = "test helpers fail the test by panicking")]

use std::time::Duration;

use mini_racer_engine::{
    ContextConfig, ContextHandle, EngineError, Host, JsErrorKind, WaitHandle, WaitOutcome,
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
}

fn terminated(result: Result<Vec<u8>, EngineError>) -> bool {
    matches!(result, Err(EngineError::Js(js)) if matches!(js.kind, JsErrorKind::Terminated(_)))
}

/// Creates contexts on several threads, runs, stops, times out and disposes
/// them, disposing every one explicitly.
pub fn churn() {
    std::thread::scope(|scope| {
        for _ in 0..4 {
            scope.spawn(|| {
                for round in 0..25 {
                    let config = ContextConfig {
                        timeout: (round % 5 == 0).then(|| Duration::from_millis(20)),
                    };
                    let context = ContextHandle::spawn(config, &mut Parking).expect("spawn");
                    assert!(
                        context
                            .eval("[1, 'two', 3.5]".to_owned(), &mut Parking)
                            .is_ok()
                    );
                    if round % 5 == 0 {
                        assert!(terminated(
                            context.eval("while (true) {}".to_owned(), &mut Parking)
                        ));
                    }
                    if round % 3 == 0 {
                        context.stop().expect("live context");
                        assert!(terminated(context.eval("1".to_owned(), &mut Parking)));
                    }
                    context.begin_dispose();
                    context.dispose(&mut Parking).expect("dispose");
                }
            });
        }
    });
}
