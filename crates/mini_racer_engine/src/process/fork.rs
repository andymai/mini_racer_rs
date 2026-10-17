use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock, Weak};
use std::time::{Duration, Instant};

use mini_racer_sync::{ForkGate, Phase, lock, wait, wait_timeout};

use crate::control::ControlBlock;
use crate::process::code_cache::CodeCache;
use crate::process::tasks::TaskRegistry;
use crate::process::watchdog::Watchdog;

// Long enough for any runner to answer and go idle, short enough that a fork
// that cannot be quiesced is merely slow.
const QUIESCE: Duration = Duration::from_millis(500);
const POLL: Duration = Duration::from_micros(200);
// A runner still running after this is interrupted to park in JavaScript.
const INTERRUPT_AFTER: Duration = Duration::from_millis(50);
// How long V8's worker threads get to finish the tasks ahead of the fork's.
const HOLD: Duration = Duration::from_millis(100);
// How often a fork posts another holding task while workers are still free.
const REPOST: Duration = Duration::from_millis(1);

// Forks since the engine's fork handlers were registered, counted by the child
// handler. A process is told apart from its ancestors by this, never by pid,
// which a descendant may get again once an ancestor has exited.
static EPOCH: AtomicU64 = AtomicU64::new(0);

/// Which process of a fork lineage this is.
#[must_use]
pub fn epoch() -> u64 {
    EPOCH.load(Ordering::SeqCst)
}

// One process of a fork lineage. A child makes its own on first use and never
// locks or frees what it inherited, which a thread that is gone may hold.
pub struct Generation {
    epoch: u64,
    // Whether every fork that led to this process found its runners idle: a
    // runner caught mid-change by any of them may have left V8's process-wide
    // state locked for every process after it. True for the first process.
    quiesced: bool,
    prepared: AtomicBool,
    gate: ForkGate,
    // Held from the prepare handler to the parent handler, so forks from
    // threads outside Ruby take turns with Ruby's.
    forking: Mutex<()>,
    blocks: Mutex<Vec<Weak<ControlBlock>>>,
    // Per process, as V8's worker threads post from the parent only, and one
    // may have held this lock at the fork.
    tasks: TaskRegistry,
    watchdog: Mutex<Option<Arc<Watchdog>>>,
    workers: Workers,
    code_cache: CodeCache,
    next: OnceLock<Box<Self>>,
}

static ROOT: OnceLock<Generation> = OnceLock::new();

impl Generation {
    fn new(epoch: u64, quiesced: bool, code_cache: CodeCache) -> Self {
        Self {
            epoch,
            quiesced,
            prepared: AtomicBool::new(false),
            gate: ForkGate::default(),
            forking: Mutex::new(()),
            blocks: Mutex::new(Vec::new()),
            tasks: TaskRegistry::default(),
            watchdog: Mutex::new(None),
            workers: Workers::default(),
            code_cache,
            next: OnceLock::new(),
        }
    }

    pub const fn quiesced(&self) -> bool {
        self.quiesced
    }

    pub(crate) const fn tasks(&self) -> &TaskRegistry {
        &self.tasks
    }

    pub(crate) const fn watchdog(&self) -> &Mutex<Option<Arc<Watchdog>>> {
        &self.watchdog
    }

    #[must_use]
    pub const fn code_cache(&self) -> &CodeCache {
        &self.code_cache
    }

    pub fn register(&self, block: &Arc<ControlBlock>) {
        let mut blocks = lock(&self.blocks);
        blocks.retain(|block| block.strong_count() > 0);
        blocks.push(Arc::downgrade(block));
    }

    /// Called by a runner about to use its isolate: once a fork began, it
    /// waits without the isolate until the fork is over.
    pub fn resume(&self, block: &ControlBlock) {
        self.gate.resume(&block.phase);
    }

    /// Waits out a pending fork; a runner interrupted in JavaScript holds its
    /// isolate meanwhile, and the child leaves that context alone.
    pub fn pause(&self, block: &ControlBlock) {
        self.gate.pause(&block.phase);
    }
}

pub fn current() -> &'static Generation {
    let epoch = epoch();
    let mut generation = ROOT.get_or_init(|| Generation::new(epoch, true, CodeCache::default()));
    while generation.epoch != epoch {
        let quiesced = generation.quiesced && generation.prepared.load(Ordering::SeqCst);
        let next = generation.epoch.saturating_add(1);
        let parent = generation;
        generation = parent.next.get_or_init(|| {
            Box::new(Generation::new(
                next,
                quiesced,
                parent.code_cache.inherited(),
            ))
        });
    }
    generation
}

// Forks from Ruby under way, counted so nested `_fork` hooks and Ruby threads
// that move between native threads keep it right. Forks while it is zero run
// no Ruby in the child (spawn, system and backticks on macOS) and leave the
// runners be.
static INTENDED: AtomicUsize = AtomicUsize::new(0);
// Whether a fork from Ruby was under way when the latest fork was prepared.
// Its child starts its own count from this: only the forking thread carries on
// there, inside the one `_fork` hook it forked from, if any.
static FORK_INTENDED: AtomicBool = AtomicBool::new(false);

/// Marks the forks made until the matching `intend(false)` as ones whose child
/// runs Ruby and may use the contexts it inherits.
pub fn intend(forking: bool) {
    if forking {
        INTENDED.fetch_add(1, Ordering::SeqCst);
    } else {
        let mut count = INTENDED.load(Ordering::SeqCst);
        while count > 0 {
            match INTENDED.compare_exchange_weak(
                count,
                count.saturating_sub(1),
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                Ok(_) => break,
                Err(now) => count = now,
            }
        }
    }
}

// The fork this thread is preparing, so its parent handler alone reopens the
// gate. A child inherits its forking thread's turn, which belongs to the
// parent's generation.
type Turn = (&'static Generation, MutexGuard<'static, ()>);

// A turn taken in an ancestor process: its lock is left held, as the child
// releases nothing it inherited.
const fn abandon(turn: Turn) {
    #[expect(
        clippy::mem_forget,
        reason = "the lock belongs to an ancestor's generation"
    )]
    std::mem::forget(turn);
}

thread_local! {
    static FORKING: RefCell<Option<Turn>> = const { RefCell::new(None) };
}

// pthread_atfork's prepare handler, on the forking thread inside fork() and
// after every Ruby `_fork` hook, so no Ruby code runs while runners are held.
// It waits until every runner is idle or parked, then until each of V8's
// worker threads runs a task of the fork's that holds it until the parent
// handler, so the child finds no isolate mid-change and
// V8's worker queue neither locked nor waited on. No runner needs Ruby to get
// there: one waiting on a callback already counts as parked. Past the bounds
// the fork goes ahead anyway.
pub(crate) fn prepare() {
    let intended = INTENDED.load(Ordering::SeqCst) != 0;
    FORK_INTENDED.store(intended, Ordering::SeqCst);
    if !intended {
        return;
    }
    let generation = current();
    let turn = lock(&generation.forking);
    if let Some(stale) = FORKING.with(|forking| forking.borrow_mut().replace((generation, turn))) {
        abandon(stale);
    }
    // Begun before the registry is read: a runner registered later sees it.
    generation.gate.begin();
    let blocks: Vec<Arc<ControlBlock>> = lock(&generation.blocks)
        .iter()
        .filter_map(Weak::upgrade)
        .collect();
    let started = Instant::now();
    let mut interrupted = false;
    let quiesced = loop {
        let running: Vec<&Arc<ControlBlock>> = blocks
            .iter()
            .filter(|block| block.phase.get() == Phase::Running)
            .collect();
        if running.is_empty() {
            break true;
        }
        let waited = started.elapsed();
        if waited >= QUIESCE {
            break false;
        }
        if !interrupted && waited >= INTERRUPT_AFTER {
            interrupted = true;
            for block in running {
                block.interrupt_for_fork();
            }
        }
        std::thread::sleep(POLL);
    };
    let held = generation.workers.hold(Instant::now() + HOLD);
    generation
        .prepared
        .store(quiesced && held, Ordering::SeqCst);
}

// V8's worker threads, each held in a task of a fork's for its length. A
// waiting worker is queued on V8's condition variable with its record on its
// own stack, which a child need not keep: the child's first post to the
// workers then reads freed memory. A worker inside a task is not queued and
// holds no lock of V8's.
#[derive(Default)]
struct Workers {
    hold: Mutex<Hold>,
    changed: Condvar,
}

#[derive(Default)]
struct Hold {
    // The fork whose tasks hold workers; a task left from an earlier one
    // returns at once.
    fork: u64,
    holding: bool,
    held: usize,
}

impl Workers {
    // Returns whether every worker was held by `deadline`.
    fn hold(&'static self, deadline: Instant) -> bool {
        let Some(platform) = crate::process::platform_with_workers() else {
            return true;
        };
        let count = crate::sys::worker_threads(platform);
        let fork = {
            let mut hold = lock(&self.hold);
            hold.fork += 1;
            hold.holding = true;
            hold.held = 0;
            hold.fork
        };
        let post =
            || crate::sys::post_on_worker_thread(platform, Box::new(move || self.keep(fork)));
        for _ in 0..count {
            post();
        }
        let mut hold = lock(&self.hold);
        let all = loop {
            if hold.held >= count {
                break true;
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break false;
            }
            hold = wait_timeout(&self.changed, hold, left.min(REPOST));
            // V8 wakes one idle worker per post, from a list that keeps a
            // worker whose timed wait for a delayed task expired, so a post
            // can wake no one. Another post per slice reaches the rest; a
            // spare one returns once the fork is over.
            if hold.held < count {
                drop(hold);
                post();
                hold = lock(&self.hold);
            }
        };
        drop(hold);
        all
    }

    fn keep(&self, fork: u64) {
        let mut hold = lock(&self.hold);
        if hold.fork != fork || !hold.holding {
            return;
        }
        hold.held += 1;
        self.changed.notify_all();
        while hold.fork == fork && hold.holding {
            hold = wait(&self.changed, hold);
        }
        drop(hold);
    }

    fn release(&self) {
        lock(&self.hold).holding = false;
        self.changed.notify_all();
    }
}

// pthread_atfork's parent handler, whether or not the fork succeeded.
pub(crate) fn parent() {
    let Some((generation, turn)) = FORKING.with(|forking| forking.borrow_mut().take()) else {
        return;
    };
    if generation.epoch != epoch() {
        return abandon((generation, turn));
    }
    // Cleared before the gate opens, so no fork from another thread copies
    // a prepared flag while runners move.
    generation.prepared.store(false, Ordering::SeqCst);
    generation.workers.release();
    generation.gate.end();
    drop(turn);
}

// pthread_atfork's child handler: atomics only, which is all a child may touch
// before it execs or returns to Ruby. Everything else the child reads lazily
// from what it inherited.
pub(crate) fn child() {
    EPOCH.fetch_add(1, Ordering::SeqCst);
    INTENDED.store(
        usize::from(FORK_INTENDED.load(Ordering::SeqCst)),
        Ordering::SeqCst,
    );
}

/// Taken by every test that holds or counts V8's worker threads, so tests
/// sharing a process take turns with them.
#[cfg(test)]
pub(crate) static WORKER_TESTS: Mutex<()> = Mutex::new(());

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, MutexGuard};
    use std::time::{Duration, Instant};

    use mini_racer_sync::lock;

    use super::{WORKER_TESTS, Workers};

    fn workers() -> (MutexGuard<'static, ()>, &'static Workers) {
        let turn = lock(&WORKER_TESTS);
        crate::process::ensure_initialized();
        (turn, Box::leak(Box::default()))
    }

    // Held, then let go: a task posted afterwards runs.
    #[test]
    fn holds_every_worker_until_released() {
        let (_turn, workers) = workers();
        assert!(workers.hold(Instant::now() + Duration::from_secs(5)));
        workers.release();
        let platform = crate::process::platform_with_workers().expect("initialized");
        let ran = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&ran);
        crate::sys::post_on_worker_thread(
            platform,
            Box::new(move || flag.store(true, Ordering::SeqCst)),
        );
        let deadline = Instant::now() + Duration::from_secs(5);
        while !ran.load(Ordering::SeqCst) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(ran.load(Ordering::SeqCst));
    }

    // A worker whose timed wait for a delayed task expired stays on V8's
    // idle list, so one post per worker misses one.
    #[test]
    fn holds_every_worker_after_a_delayed_task() {
        let (_turn, workers) = workers();
        let platform = crate::process::platform_with_workers().expect("initialized");
        std::thread::sleep(Duration::from_millis(50));
        crate::sys::post_delayed_on_worker_thread(platform, 0.05, Box::new(|| {}));
        std::thread::sleep(Duration::from_millis(200));
        assert!(workers.hold(Instant::now() + Duration::from_secs(5)));
        workers.release();
    }

    // A task left from a fork that timed out returns at once in a later one.
    #[test]
    fn a_task_of_an_earlier_fork_holds_nothing() {
        let (_turn, workers) = workers();
        assert!(!workers.hold(Instant::now()));
        workers.release();
        assert!(workers.hold(Instant::now() + Duration::from_secs(5)));
        workers.release();
    }
}
