#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "test helpers fail the test by panicking"
)]

use std::fmt::Write;
use std::time::Duration;

use mini_racer_engine::{
    ContextConfig, ContextHandle, EngineError, Host, JsErrorKind, SnapshotBlob, WaitHandle,
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
}

fn terminated(result: Result<Vec<u8>, EngineError>) -> bool {
    matches!(result, Err(EngineError::Js(js)) if matches!(js.kind, JsErrorKind::Terminated(_)))
}

// Every fourth context starts from one snapshot blob the threads share, every
// seventh loads one large script, read from the source contexts share, and
// every sixth round a context reaches its heap limit, so the leak checks
// cover all three.
pub fn churn() {
    let built = ContextHandle::create_snapshot("var shared = 1".into(), None, &mut Parking)
        .expect("snapshot");
    let blob = SnapshotBlob::new(&built).expect("a blob V8 wrote");
    let mut bundle = String::from("// \u{1F600}\n");
    for i in 0..25_000 {
        writeln!(bundle, "function churned{i}(a) {{ return a + {i}; }}").expect("write");
    }
    let bundle = &bundle;
    std::thread::scope(|scope| {
        for _ in 0..4 {
            let blob = blob.clone();
            scope.spawn(move || {
                for round in 0..25 {
                    let config = ContextConfig {
                        snapshot: (round % 4 == 0).then(|| blob.clone()),
                        ..ContextConfig::default()
                    };
                    let context = ContextHandle::spawn(config, &mut Parking).expect("spawn");
                    if round % 7 == 3 {
                        context
                            .eval("bundle.js".to_owned(), bundle.clone(), &mut Parking)
                            .expect("eval");
                    }
                    // The delayed task is still queued at the dispose.
                    assert!(
                        context
                            .eval(
                                "<eval>".to_owned(),
                                "Atomics.waitAsync(new Int32Array(new SharedArrayBuffer(4)), 0, 0, 60000); \
                                 [1, 'two', 3.5]"
                                    .to_owned(),
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
                    if round % 6 == 1 {
                        let capped = ContextHandle::spawn(
                            ContextConfig {
                                heap_limit: Some(32 << 20),
                                ..ContextConfig::default()
                            },
                            &mut Parking,
                        )
                        .expect("spawn");
                        assert!(terminated(capped.eval(
                            "<eval>".to_owned(),
                            "const a = []; for (;;) a.push({i: a.length, s: 'k' + a.length})"
                                .to_owned(),
                            &mut Parking
                        )));
                        capped.begin_dispose();
                        capped.dispose(&mut Parking).expect("dispose");
                    }
                    if round % 5 == 0 {
                        let timed = ContextHandle::spawn(
                            ContextConfig {
                                timeout: Some(Duration::from_millis(20)),
                                ..ContextConfig::default()
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
        drop(blob);
    });
}
