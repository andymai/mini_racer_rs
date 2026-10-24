use crate::lock::{lock, wait};
use crate::primitives::{AtomicBool, AtomicU8, Condvar, Mutex, Ordering};

/// Where a runner is, as a fork sees it. Only `Running` keeps a fork waiting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Phase {
    Running = 0,
    /// Idle, with its isolate in the control block and no lock held.
    Deposited = 1,
    /// Holding its isolate with no V8 call under way: waiting for a request,
    /// inside a callback waiting for Ruby, or awaiting a promise until a task
    /// is due.
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

// Both sides of the gate's handshake are read-modify-writes of the phase, so
// whichever comes second in its order sees the other: ordering that rests on
// a store and a load alone is beyond what loom checks.
impl RunnerPhase {
    pub fn set(&self, phase: Phase) {
        self.0.swap(phase as u8, Ordering::SeqCst);
    }

    /// For a forker or a forked child; a runner knows its own phase.
    pub fn get(&self) -> Phase {
        match self.0.fetch_add(0, Ordering::SeqCst) {
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
/// A runner publishes `Running` and then reads `begun`, and `begin` sets
/// `begun` before the forker reads any phase, so either the runner sees the
/// fork and waits or the forker sees it `Running` and waits for it. Every runner resumes before every request, so the
/// common case takes no lock: one lock shared by all of a process's runners
/// cost three quarters of the call rate of four busy threads.
pub struct ForkGate {
    begun: AtomicBool,
    pending: Mutex<bool>,
    released: Condvar,
}

impl Default for ForkGate {
    fn default() -> Self {
        Self {
            begun: AtomicBool::new(false),
            pending: Mutex::new(false),
            released: Condvar::new(),
        }
    }
}

impl ForkGate {
    pub fn begin(&self) {
        let mut pending = lock(&self.pending);
        *pending = true;
        self.begun.store(true, Ordering::SeqCst);
        drop(pending);
    }

    pub fn end(&self) {
        let mut pending = lock(&self.pending);
        *pending = false;
        self.begun.store(false, Ordering::SeqCst);
        drop(pending);
        self.released.notify_all();
    }

    pub fn resume(&self, phase: &RunnerPhase) {
        phase.set(Phase::Running);
        if !self.begun.load(Ordering::SeqCst) {
            return;
        }
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

#[cfg(test)]
#[cfg(not(loom))]
mod tests {
    use super::*;

    #[test]
    fn every_phase_reads_back_as_it_was_set() {
        let phase = RunnerPhase::default();
        assert_eq!(phase.get(), Phase::Running);
        for set in [
            Phase::Deposited,
            Phase::Parked,
            Phase::ParkedForFork,
            Phase::Exited,
            Phase::Running,
        ] {
            phase.set(set);
            assert_eq!(phase.get(), set);
        }
    }
}
