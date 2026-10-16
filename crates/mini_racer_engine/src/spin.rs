use std::time::{Duration, Instant};

/// Spins until `ready` or the budget runs out; returns whether it became ready.
pub fn spin_until(budget: Duration, mut ready: impl FnMut() -> bool) -> bool {
    if ready() {
        return true;
    }
    if budget.is_zero() {
        return false;
    }
    let start = Instant::now();
    loop {
        for _ in 0..32 {
            std::hint::spin_loop();
            if ready() {
                return true;
            }
        }
        if start.elapsed() >= budget {
            return ready();
        }
    }
}
