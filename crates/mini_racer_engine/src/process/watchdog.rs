use std::cmp::{Ordering, Reverse};
use std::collections::BinaryHeap;
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::time::{Duration, Instant};

use mini_racer_sync::{DispatchId, Interrupt, lock, wait, wait_timeout};

use crate::EngineError;
use crate::control::{ControlBlock, Timer};

struct Entry {
    at: Instant,
    seq: u64,
    block: Weak<ControlBlock>,
}

impl PartialEq for Entry {
    fn eq(&self, other: &Self) -> bool {
        (self.at, self.seq) == (other.at, other.seq)
    }
}

impl Eq for Entry {}

impl PartialOrd for Entry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Entry {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.at, self.seq).cmp(&(other.at, other.seq))
    }
}

#[derive(Default)]
struct Timers {
    heap: BinaryHeap<Reverse<Entry>>,
    seq: u64,
}

pub struct Watchdog {
    timers: Mutex<Timers>,
    changed: Condvar,
}

impl Watchdog {
    fn push(&self, at: Instant, block: Weak<ControlBlock>) {
        let mut timers = lock(&self.timers);
        timers.seq = timers.seq.wrapping_add(1);
        let seq = timers.seq;
        timers.heap.push(Reverse(Entry { at, seq, block }));
        drop(timers);
        self.changed.notify_one();
    }
}

// One thread per process serves every context's timeouts. It is kept per
// fork generation: a forked child has the parent's watchdog state but not its
// thread, and a runner of the parent may have held its lock at the fork.
fn current() -> Result<Arc<Watchdog>, EngineError> {
    let mut slot = lock(super::fork::current().watchdog());
    if let Some(watchdog) = slot.as_ref() {
        return Ok(Arc::clone(watchdog));
    }
    let watchdog = Arc::new(Watchdog {
        timers: Mutex::default(),
        changed: Condvar::new(),
    });
    let serving = Arc::clone(&watchdog);
    std::thread::Builder::new()
        .name("mini_racer_rs watchdog".to_owned())
        .spawn(move || run(&serving))
        .map_err(|e| EngineError::Internal(format!("cannot start the timeout watchdog: {e}")))?;
    *slot = Some(Arc::clone(&watchdog));
    drop(slot);
    Ok(watchdog)
}

/// Arms `dispatch`'s timeout. Fails rather than let a timed script run
/// unbounded when the watchdog cannot start.
// A context's timer slot holds its current deadline; a heap entry only wakes
// the watchdog to look at the slot, so a context needs a new entry only when
// its deadline moves earlier.
pub fn schedule(
    after: Duration,
    block: &Arc<ControlBlock>,
    dispatch: DispatchId,
) -> Result<(), EngineError> {
    arm(after, block, |timer, at| timer.due = Some((at, dispatch)))
}

/// Arms the timeout a nested script runs under on its own.
pub fn schedule_nested(
    after: Duration,
    block: &Arc<ControlBlock>,
    id: DispatchId,
) -> Result<(), EngineError> {
    arm(after, block, |timer, at| timer.nested.push((at, id)))
}

fn arm(
    after: Duration,
    block: &Arc<ControlBlock>,
    set: impl FnOnce(&mut Timer, Instant),
) -> Result<(), EngineError> {
    let watchdog = current()?;
    let at = Instant::now()
        .checked_add(after)
        .ok_or_else(|| EngineError::Internal("timeout out of range".to_owned()))?;
    let mut timer = lock(&block.timer);
    set(&mut timer, at);
    let wake = timer.queued.is_none_or(|queued| at < queued);
    if wake {
        timer.queued = Some(at);
    }
    drop(timer);
    // A wake entry the watchdog no longer needs is harmless: it rereads the slot.
    if wake {
        watchdog.push(at, Arc::downgrade(block));
    }
    Ok(())
}

/// Whether the outermost dispatch's own timeout, or a nested script's, fires
/// within `after` from now.
pub fn ends_within(block: &ControlBlock, after: Duration) -> bool {
    let Some(deadline) = Instant::now().checked_add(after) else {
        return true;
    };
    let timer = lock(&block.timer);
    timer
        .due
        .iter()
        .chain(&timer.nested)
        .any(|(at, _)| *at <= deadline)
}

/// Bounds work `dispatch` does after its timeout fired with the timeout again.
/// A deadline that has not fired yet already bounds it.
pub fn rearm_if_fired(
    after: Duration,
    block: &Arc<ControlBlock>,
    dispatch: DispatchId,
) -> Result<(), EngineError> {
    let pending = lock(&block.timer)
        .due
        .is_some_and(|(_, due)| due == dispatch);
    if pending {
        return Ok(());
    }
    schedule(after, block, dispatch)
}

/// Forgets the context's pending timeout once its dispatch is over.
pub fn clear(block: &ControlBlock, dispatch: DispatchId) {
    let mut timer = lock(&block.timer);
    if timer.due.is_some_and(|(_, due)| due == dispatch) {
        timer.due = None;
    }
}

/// Forgets a nested script's own timeout once the script is over.
pub fn clear_nested(block: &ControlBlock, id: DispatchId) {
    lock(&block.timer).nested.retain(|(_, due)| *due != id);
}

fn run(watchdog: &Watchdog) {
    loop {
        let mut timers = lock(&watchdog.timers);
        let now = Instant::now();
        match timers.heap.peek().map(|Reverse(entry)| entry.at) {
            None => drop(wait(&watchdog.changed, timers)),
            Some(at) if at > now => {
                drop(wait_timeout(
                    &watchdog.changed,
                    timers,
                    at.saturating_duration_since(now),
                ));
            }
            Some(_) => {
                let mut due = Vec::new();
                while timers
                    .heap
                    .peek()
                    .is_some_and(|Reverse(entry)| entry.at <= now)
                {
                    if let Some(Reverse(entry)) = timers.heap.pop() {
                        due.push(entry.block);
                    }
                }
                // Lock order: the heap is released before any context's timer
                // or termination cell is taken.
                drop(timers);
                for block in due.iter().filter_map(Weak::upgrade) {
                    fire(watchdog, &block, now);
                }
            }
        }
    }
}

fn fire(watchdog: &Watchdog, block: &Arc<ControlBlock>, now: Instant) {
    let mut timer = lock(&block.timer);
    let mut fired = Vec::new();
    if let Some((at, id)) = timer.due
        && at <= now
    {
        timer.due = None;
        fired.push(id);
    }
    timer.nested.retain(|&(at, id)| {
        let due = at <= now;
        if due {
            fired.push(id);
        }
        !due
    });
    let next = timer
        .due
        .iter()
        .chain(&timer.nested)
        .map(|(at, _)| *at)
        .min();
    timer.queued = next;
    drop(timer);
    for id in fired {
        block.term.request(Interrupt::Timeout(id));
    }
    if let Some(at) = next {
        watchdog.push(at, Arc::downgrade(block));
    }
}
