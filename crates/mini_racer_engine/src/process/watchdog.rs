use std::cmp::{Ordering, Reverse};
use std::collections::BinaryHeap;
use std::sync::{Arc, Condvar, Mutex, Once, OnceLock, Weak};
use std::time::{Duration, Instant};

use mini_racer_sync::{DispatchId, Interrupt, lock, wait, wait_timeout};

use crate::control::ControlBlock;

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

#[derive(Default)]
struct Watchdog {
    timers: Mutex<Timers>,
    changed: Condvar,
}

static WATCHDOG: OnceLock<Watchdog> = OnceLock::new();
static STARTED: Once = Once::new();

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

// One thread serves every context's timeouts. A context's timer slot holds its
// current deadline; a heap entry only wakes the watchdog to look at the slot, so
// a context needs a new entry only when its deadline moves earlier.
// TODO: spawn per fork generation once the fork protocol lands; a forked child
// inherits a started flag without the thread, so its timeouts never fire.
pub fn schedule(after: Duration, block: &Arc<ControlBlock>, dispatch: DispatchId) {
    let Some(at) = Instant::now().checked_add(after) else {
        return;
    };
    let watchdog = WATCHDOG.get_or_init(Watchdog::default);
    STARTED.call_once(|| {
        // Without the thread the dispatch runs unbounded, as mini_racer does when
        // it cannot start its watchdog.
        let _ = std::thread::Builder::new()
            .name("mini_racer_rs watchdog".to_owned())
            .spawn(|| run(watchdog));
    });
    let mut timer = lock(&block.timer);
    timer.due = Some((at, dispatch));
    let wake = timer.queued.is_none_or(|queued| at < queued);
    if wake {
        timer.queued = Some(at);
    }
    drop(timer);
    // A wake entry the watchdog no longer needs is harmless: it rereads the slot.
    if wake {
        watchdog.push(at, Arc::downgrade(block));
    }
}

/// Forgets the context's pending timeout once its dispatch is over.
pub fn clear(block: &ControlBlock, dispatch: DispatchId) {
    let mut timer = lock(&block.timer);
    if timer.due.is_some_and(|(_, due)| due == dispatch) {
        timer.due = None;
    }
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
    timer.queued = None;
    let due = timer.due;
    match due {
        Some((at, _)) if at <= now => timer.due = None,
        Some((at, _)) => timer.queued = Some(at),
        None => {}
    }
    drop(timer);
    match due {
        Some((at, dispatch)) if at <= now => {
            block.term.request(Interrupt::Timeout(dispatch));
        }
        Some((at, _)) => watchdog.push(at, Arc::downgrade(block)),
        None => {}
    }
}
