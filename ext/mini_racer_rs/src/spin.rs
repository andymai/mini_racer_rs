use std::cell::Cell;
use std::sync::OnceLock;
use std::time::Duration;

/// How long a Ruby thread spins for a runner's answer before it gives up the
/// GVL, and then before it parks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Budget {
    pub gvl: Duration,
    pub nogvl: Duration,
}

impl Budget {
    fn parse(setting: &str) -> Self {
        let parts: Vec<Option<u64>> = setting
            .split(',')
            .map(|part| part.trim().parse().ok())
            .collect();
        let at = |i: usize, default: u64| {
            Duration::from_micros(parts.get(i).copied().flatten().unwrap_or(default))
        };
        Self {
            gvl: at(0, 20),
            nogvl: at(1, 50),
        }
    }

    pub const fn total(&self) -> Duration {
        self.gvl.saturating_add(self.nogvl)
    }
}

// Unstable tuning knob: MINI_RACER_RS_SPIN_US=<with GVL>,<without GVL>.
pub fn budget() -> &'static Budget {
    static BUDGET: OnceLock<Budget> = OnceLock::new();
    BUDGET
        .get_or_init(|| Budget::parse(&std::env::var("MINI_RACER_RS_SPIN_US").unwrap_or_default()))
}

// A context whose answers outlast the budget, such as one rendering pages,
// gains nothing from spinning: each wait burns the whole budget on the
// caller's CPU, the first part holding the GVL from other threads, and then
// parks anyway. A few in a row are enough to tell it from a one-off.
const STREAK: u8 = 3;

/// One context's recent waits: after `STREAK` in a row outlast the budget its
/// waits park at once, until one is answered within the budget.
#[derive(Debug, Default)]
pub struct History {
    slow: Cell<u8>,
}

impl History {
    pub const fn spins(&self) -> bool {
        self.slow.get() < STREAK
    }

    pub fn record(&self, within_budget: bool) {
        let slow = if within_budget {
            0
        } else {
            self.slow.get().saturating_add(1).min(STREAK)
        };
        self.slow.set(slow);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_budget_defaults_and_reads_the_knob() {
        let us = Duration::from_micros;
        assert_eq!(
            Budget::parse(""),
            Budget {
                gvl: us(20),
                nogvl: us(50)
            }
        );
        assert_eq!(
            Budget::parse("0,0"),
            Budget {
                gvl: us(0),
                nogvl: us(0)
            }
        );
        assert_eq!(
            Budget::parse("5, x"),
            Budget {
                gvl: us(5),
                nogvl: us(50)
            }
        );
        assert_eq!(Budget::parse("").total(), us(70));
    }

    #[test]
    fn a_streak_of_slow_answers_stops_spinning() {
        let history = History::default();
        for _ in 1..STREAK {
            history.record(false);
            assert!(history.spins());
        }
        history.record(false);
        assert!(!history.spins());
        history.record(false);
        assert!(!history.spins());
    }

    #[test]
    fn one_answer_within_the_budget_brings_spinning_back() {
        let history = History::default();
        for _ in 0..STREAK {
            history.record(false);
        }
        history.record(true);
        assert!(history.spins());
        history.record(false);
        assert!(history.spins());
    }
}
