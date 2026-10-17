// Its own test binary: it reads a count kept for the whole process.

use mini_racer_engine::process::unserved_tasks;
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

// Were V8 to post while an isolate is made, before its runner registers it,
// the task would be lost, and an await could wait on it forever. The script
// has V8 post a delayed task, which must reach the runner, and leaves another
// pending for the teardown.
#[test]
fn a_context_gets_no_task_before_its_runner_registers_it() {
    let waiting = "Atomics.waitAsync(new Int32Array(new SharedArrayBuffer(4)), 0, 0";
    for _ in 0..3 {
        let context = ContextHandle::spawn(ContextConfig::default(), &mut Parking).expect("spawn");
        context
            .eval(
                "<eval>",
                format!("{waiting}, 1); {waiting}, 60000); 0"),
                &mut Parking,
            )
            .expect("eval");
        std::thread::sleep(std::time::Duration::from_millis(20));
        assert!(context.pump_message_loop(&mut Parking).expect("pump"));
        context.begin_dispose();
        context.dispose(&mut Parking).expect("dispose");
    }
    assert_eq!(unserved_tasks(), 0);
}
