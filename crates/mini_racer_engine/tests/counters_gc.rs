#![allow(clippy::panic, reason = "test helpers fail the test by panicking")]

use mini_racer_engine::process::platform;
use mini_racer_engine::{ContextConfig, ContextHandle, Host, WaitHandle, WaitOutcome};

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

// `gc()` needs a flag, which freezes when V8 initializes, so this test has a
// process of its own. Each call runs one collection of the kind it names.
#[test]
fn each_collection_counts_once() {
    platform::push_flag(b"expose_gc", None).expect("flag");
    let context = ContextHandle::spawn(ContextConfig::default(), &mut Parking).expect("spawn");
    let collected = |source: &str| {
        let before = context.counters();
        context
            .eval("<eval>", source, &mut Parking)
            .expect("collects");
        let after = context.counters();
        assert!(after.gc_ns > before.gc_ns, "{source}: {after:?}");
        after.gc_count - before.gc_count
    };

    assert_eq!(collected("gc(); 0"), 1);
    assert_eq!(collected("gc(); gc(); gc(); 0"), 3);
    assert_eq!(collected("gc({ type: 'minor' }); 0"), 1);
    assert_eq!(
        collected("gc({ type: 'major' }); gc({ type: 'minor' }); 0"),
        2
    );
}
