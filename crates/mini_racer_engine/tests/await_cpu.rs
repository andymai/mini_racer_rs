#![cfg(target_os = "linux")]

// Its own test binary: it reads the CPU time of the whole process, which
// tests running alongside it in one binary would add to.

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

// User and system CPU time of this process, in clock ticks.
fn cpu_ticks() -> u64 {
    let stat = std::fs::read_to_string("/proc/self/stat").expect("stat");
    let after_name = stat.rfind(')').expect("process name") + 2;
    let fields: Vec<&str> = stat[after_name..].split(' ').collect();
    let ticks = |index: usize| fields[index].parse::<u64>().expect("a tick count");
    ticks(11) + ticks(12)
}

// The runner sleeps until the task is due: an await that spun instead would
// spend most of the 300 ms on the CPU.
#[test]
fn an_await_sleeps_rather_than_spins() {
    let context = ContextHandle::spawn(ContextConfig::default(), &mut Parking).expect("spawn");
    let before = cpu_ticks();
    let settled = context.eval_await(
        "<eval>",
        "Atomics.waitAsync(new Int32Array(new SharedArrayBuffer(4)), 0, 0, 300).value.then(() => 1)",
        &mut Parking,
    );
    assert!(settled.is_ok(), "{settled:?}");
    let spent = cpu_ticks() - before;
    assert!(spent < 10, "the await used {spent} ticks of CPU");
}
