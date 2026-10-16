use std::sync::OnceLock;
use std::time::Duration;

use magnus::{Error, Ruby};
use mini_racer_engine::{Host, WaitHandle, WaitOutcome};

use crate::{errors, rb_raw};

struct SpinBudget {
    gvl: Duration,
    nogvl: Duration,
}

// Unstable tuning knob: MINI_RACER_RS_SPIN_US=<with GVL>,<without GVL>.
fn budget() -> &'static SpinBudget {
    static BUDGET: OnceLock<SpinBudget> = OnceLock::new();
    BUDGET.get_or_init(|| {
        let setting = std::env::var("MINI_RACER_RS_SPIN_US").unwrap_or_default();
        let parts: Vec<Option<u64>> = setting
            .split(',')
            .map(|part| part.trim().parse().ok())
            .collect();
        let at = |i: usize, default: u64| {
            Duration::from_micros(parts.get(i).copied().flatten().unwrap_or(default))
        };
        SpinBudget {
            gvl: at(0, 20),
            nogvl: at(1, 50),
        }
    })
}

/// Waits for a runner from a Ruby thread: spins holding the GVL, then releases
/// it, spins and parks. A Ruby interrupt wakes the park; traps run and the wait
/// goes on, while an exception (Timeout, Thread#raise, #kill) cancels the
/// dispatch and is raised once the engine has its response.
pub struct RubyHost<'r> {
    ruby: &'r Ruby,
    pending: Option<Error>,
}

impl<'r> RubyHost<'r> {
    pub const fn new(ruby: &'r Ruby) -> Self {
        Self {
            ruby,
            pending: None,
        }
    }

    /// The Ruby exception that cancelled the dispatch; it outranks whatever the
    /// engine returned.
    pub fn finish<T>(self, result: Result<T, Error>) -> Result<T, Error> {
        self.pending.map_or(result, Err)
    }
}

impl Host for RubyHost<'_> {
    fn wait(&mut self, wait: &WaitHandle<'_>) -> WaitOutcome {
        let budget = budget();
        if wait.spin(budget.gvl) {
            return WaitOutcome::Ready;
        }
        let unparker = wait.unparker();
        loop {
            let parked = rb_raw::nogvl(
                self.ruby,
                || {
                    if !wait.spin(budget.nogvl) {
                        wait.park();
                    }
                },
                &move || unparker.unpark(),
            );
            if wait.ready() {
                return WaitOutcome::Ready;
            }
            if parked.as_ref().is_some_and(Result::is_err) {
                self.pending = Some(errors::internal(self.ruby, "panic while waiting"));
                return WaitOutcome::Cancel;
            }
            if let Err(error) = rb_raw::check_ints(self.ruby) {
                self.pending = Some(error);
                return WaitOutcome::Cancel;
            }
        }
    }

    // Interrupts cannot end this wait, but they still run: a pending one would
    // otherwise make every rb_nogvl return at once and keep the GVL held. An
    // exception they raise is kept and raised once the dispatch is over.
    fn wait_blocking(&mut self, wait: &WaitHandle<'_>) {
        while !wait.ready() {
            let parked = rb_raw::nogvl(
                self.ruby,
                || {
                    while !wait.ready() {
                        wait.park();
                    }
                },
                &|| {},
            );
            if parked.is_none()
                && let Err(error) = rb_raw::check_ints(self.ruby)
            {
                self.pending.get_or_insert(error);
            }
        }
    }
}
