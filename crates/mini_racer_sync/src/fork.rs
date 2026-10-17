use crate::lock::{lock, wait};
use crate::primitives::{AtomicU8, Condvar, Mutex, Ordering};

/// Where a runner is, as a fork sees it. Only `Running` keeps a fork waiting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Phase {
    Running = 0,
    /// Idle, with its isolate in the control block and no lock held.
    Deposited = 1,
    /// Holding its isolate with no V8 call under way: inside a callback
    /// waiting for Ruby, or awaiting a promise until a task is due.
    Parked = 2,
    /// Woken while a fork was pending, before touching its isolate, or
    /// interrupted in JavaScript to wait the fork out.
    ParkedForFork = 3,
    Exited = 4,
}

pub struct RunnerPhase(AtomicU8);

impl Default for RunnerPhase {
    fn default() -> Self {
        Self(AtomicU8::new(Phase::Running as u8))
    }
}

impl RunnerPhase {
    pub fn set(&self, phase: Phase) {
        self.0.store(phase as u8, Ordering::SeqCst);
    }

    pub fn get(&self) -> Phase {
        match self.0.load(Ordering::SeqCst) {
            1 => Phase::Deposited,
            2 => Phase::Parked,
            3 => Phase::ParkedForFork,
            4 => Phase::Exited,
            _ => Phase::Running,
        }
    }
}

/// Holds runners back from their isolates while a fork is pending.
///
/// A runner reads `pending` and publishes `Running` under one lock, which
/// `begin` also takes, so a forker that then finds no runner `Running` knows
/// none will touch an isolate until `end`.
pub struct ForkGate {
    pending: Mutex<bool>,
    released: Condvar,
}

impl Default for ForkGate {
    fn default() -> Self {
        Self {
            pending: Mutex::new(false),
            released: Condvar::new(),
        }
    }
}

impl ForkGate {
    pub fn begin(&self) {
        *lock(&self.pending) = true;
    }

    pub fn end(&self) {
        *lock(&self.pending) = false;
        self.released.notify_all();
    }

    pub fn resume(&self, phase: &RunnerPhase) {
        let mut pending = lock(&self.pending);
        while *pending {
            phase.set(Phase::ParkedForFork);
            pending = wait(&self.released, pending);
        }
        phase.set(Phase::Running);
        drop(pending);
    }

    /// Waits out a pending fork as `ParkedForFork`, for a runner interrupted
    /// in JavaScript, which keeps its isolate meanwhile.
    pub fn pause(&self, phase: &RunnerPhase) {
        let mut pending = lock(&self.pending);
        if !*pending {
            return;
        }
        phase.set(Phase::ParkedForFork);
        while *pending {
            pending = wait(&self.released, pending);
        }
        phase.set(Phase::Running);
        drop(pending);
    }
}
