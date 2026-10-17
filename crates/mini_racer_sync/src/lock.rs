use std::sync::PoisonError;
use std::time::Duration;

use crate::primitives::{Condvar, Mutex, MutexGuard};

// Recovering is the poison policy: every critical section in a protocol type is
// a single write or take, so a panic cannot leave state half-updated, and
// failing instead would strand the Ruby thread waiting on the other side.
pub fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

pub fn wait<'a, T>(condvar: &Condvar, guard: MutexGuard<'a, T>) -> MutexGuard<'a, T> {
    condvar.wait(guard).unwrap_or_else(PoisonError::into_inner)
}

pub fn wait_timeout<'a, T>(
    condvar: &Condvar,
    guard: MutexGuard<'a, T>,
    timeout: Duration,
) -> MutexGuard<'a, T> {
    condvar
        .wait_timeout(guard, timeout)
        .map_or_else(|poisoned| poisoned.into_inner().0, |(guard, _)| guard)
}

#[cfg(test)]
#[cfg(not(loom))]
mod tests {
    use super::*;

    #[test]
    #[expect(
        clippy::significant_drop_tightening,
        reason = "the guard must be held while the thread unwinds"
    )]
    fn a_panic_while_held_does_not_strand_later_lockers() {
        let mutex = Mutex::new(0);
        let unwound = std::panic::catch_unwind(|| {
            let mut guard = lock(&mutex);
            *guard = 1;
            std::panic::resume_unwind(Box::new("unwinding while the lock is held"));
        });

        assert!(unwound.is_err());
        assert!(mutex.is_poisoned());
        assert_eq!(*lock(&mutex), 1);
    }
}
