use std::time::{Duration, Instant};

use crate::lock::{lock, wait, wait_timeout};
use crate::primitives::{AtomicBool, Condvar, Mutex, Ordering};

#[derive(Debug, PartialEq, Eq)]
pub enum Outcome<T> {
    Done(T),
    Interrupted,
}

struct State<T> {
    result: Option<T>,
    interrupted: bool,
}

// One result from a runner to the thread that asked for it. An interrupt wakes
// the waiter without consuming the result, so the next wait still receives it.
pub struct Handoff<T> {
    state: Mutex<State<T>>,
    changed: Condvar,
    cancelled: AtomicBool,
}

impl<T> Default for Handoff<T> {
    fn default() -> Self {
        Self {
            state: Mutex::new(State {
                result: None,
                interrupted: false,
            }),
            changed: Condvar::new(),
            cancelled: AtomicBool::new(false),
        }
    }
}

impl<T> Handoff<T> {
    pub fn complete(&self, value: T) {
        lock(&self.state).result = Some(value);
        self.changed.notify_all();
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    pub fn interrupt(&self) {
        lock(&self.state).interrupted = true;
        self.changed.notify_all();
    }

    pub fn wait(&self) -> Outcome<T> {
        let mut state = lock(&self.state);
        loop {
            if let Some(value) = state.result.take() {
                return Outcome::Done(value);
            }
            if std::mem::take(&mut state.interrupted) {
                return Outcome::Interrupted;
            }
            state = wait(&self.changed, state);
        }
    }

    pub fn wait_for(&self, timeout: Duration) -> Option<T> {
        let deadline = Instant::now().checked_add(timeout);
        let mut state = lock(&self.state);
        loop {
            if let Some(value) = state.result.take() {
                return Some(value);
            }
            let remaining =
                deadline.map_or(timeout, |at| at.saturating_duration_since(Instant::now()));
            if remaining.is_zero() {
                return None;
            }
            state = wait_timeout(&self.changed, state, remaining);
        }
    }
}

#[cfg(all(test, not(loom)))]
mod tests {
    use std::sync::Arc;
    use std::thread;

    use super::*;

    #[test]
    fn a_completed_value_is_received_once() {
        let handoff = Handoff::default();
        handoff.complete(7);

        assert_eq!(handoff.wait(), Outcome::Done(7));
        assert_eq!(handoff.wait_for(Duration::from_millis(1)), None);
    }

    #[test]
    fn an_interrupt_keeps_the_result_for_the_next_wait() {
        let handoff = Handoff::default();
        handoff.interrupt();

        assert_eq!(handoff.wait(), Outcome::Interrupted);
        handoff.complete(7);
        assert_eq!(handoff.wait(), Outcome::Done(7));
    }

    #[test]
    fn wait_for_times_out_without_a_result() {
        let handoff = Handoff::<u8>::default();

        assert_eq!(handoff.wait_for(Duration::from_millis(5)), None);
    }

    #[test]
    fn wait_for_receives_a_result_from_another_thread() {
        let handoff = Arc::new(Handoff::default());
        let runner = Arc::clone(&handoff);
        let completer = thread::spawn(move || runner.complete(7));

        assert_eq!(handoff.wait_for(Duration::from_secs(10)), Some(7));
        assert!(completer.join().is_ok());
    }

    #[test]
    fn cancel_is_a_sticky_flag() {
        let handoff = Handoff::<u8>::default();
        assert!(!handoff.is_cancelled());
        handoff.cancel();

        assert!(handoff.is_cancelled());
    }
}
