use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, Weak};
use std::time::{Duration, Instant};

use mini_racer_sync::{ForkGate, Phase, lock};

use crate::control::ControlBlock;
use crate::process::platform::{self, Mode};

// Long enough for any runner to answer and go idle, short enough that a fork
// that cannot be quiesced is merely slow.
const QUIESCE: Duration = Duration::from_millis(500);
const POLL: Duration = Duration::from_micros(200);
// A runner still running after this is interrupted to park in JavaScript.
const INTERRUPT_AFTER: Duration = Duration::from_millis(50);

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
    next: OnceLock<Box<Self>>,
}

static ROOT: OnceLock<Generation> = OnceLock::new();

impl Generation {
    fn new(epoch: u64, quiesced: bool) -> Self {
        Self {
            epoch,
            quiesced,
            prepared: AtomicBool::new(false),
            gate: ForkGate::default(),
            forking: Mutex::new(()),
            blocks: Mutex::new(Vec::new()),
            next: OnceLock::new(),
        }
    }

    pub const fn quiesced(&self) -> bool {
        self.quiesced
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
    let mut generation = ROOT.get_or_init(|| Generation::new(epoch, true));
    while generation.epoch != epoch {
        let quiesced = generation.quiesced && generation.prepared.load(Ordering::SeqCst);
        let next = generation.epoch.saturating_add(1);
        generation = generation
            .next
            .get_or_init(|| Box::new(Generation::new(next, quiesced)));
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
// With a single-threaded V8 it waits until every runner is idle or parked, so
// the child sees no isolate mid-change. No runner needs Ruby to get there:
// one waiting on a callback already counts as parked. V8's worker threads are
// left out: `--single-threaded` gives them no tasks.
pub(crate) fn prepare() {
    let intended = INTENDED.load(Ordering::SeqCst) != 0;
    FORK_INTENDED.store(intended, Ordering::SeqCst);
    if !intended || platform::mode() != Mode::SingleThreaded {
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
    generation.prepared.store(quiesced, Ordering::SeqCst);
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
