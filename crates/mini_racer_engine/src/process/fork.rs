use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock, Weak};
use std::time::{Duration, Instant};

use mini_racer_sync::{lock, wait};

use crate::control::ControlBlock;
use crate::process::platform::{self, Mode};
use crate::sys;

// Long enough for any runner to answer and go idle, short enough that a fork
// that cannot be quiesced is merely slow.
const QUIESCE: Duration = Duration::from_millis(500);
const POLL: Duration = Duration::from_micros(200);
// A runner still running after this is interrupted to park in JavaScript.
const INTERRUPT_AFTER: Duration = Duration::from_millis(50);

/// Where a runner is, as a fork sees it. Only `Running` keeps a fork waiting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Phase {
    Running = 0,
    /// Idle, with its isolate in the control block and no lock held.
    Deposited = 1,
    /// Inside a callback, waiting for Ruby.
    ParkedInChannel = 2,
    /// Woken while a fork was pending, before touching its isolate.
    ParkedForFork = 3,
    Exited = 4,
}

#[derive(Default)]
pub struct RunnerPhase(AtomicU8);

impl RunnerPhase {
    pub fn set(&self, phase: Phase) {
        self.0.store(phase as u8, Ordering::SeqCst);
    }

    pub fn get(&self) -> Phase {
        match self.0.load(Ordering::SeqCst) {
            1 => Phase::Deposited,
            2 => Phase::ParkedInChannel,
            3 => Phase::ParkedForFork,
            4 => Phase::Exited,
            _ => Phase::Running,
        }
    }
}

// One process of a fork lineage. A child makes its own on first use and never
// locks or frees what it inherited, which a thread that is gone may hold.
pub struct Generation {
    pid: u32,
    // Whether this process came from a fork its runners were idle for: the
    // parent's `prepared`, as the child inherited it at the fork.
    quiesced: bool,
    prepared: AtomicBool,
    pending: AtomicBool,
    // The thread whose fork is pending, which must not wait on a runner.
    forker: AtomicU64,
    gate: Mutex<()>,
    released: Condvar,
    blocks: Mutex<Vec<Weak<ControlBlock>>>,
    next: OnceLock<Box<Self>>,
}

static ROOT: OnceLock<Generation> = OnceLock::new();

impl Generation {
    const fn new(pid: u32, quiesced: bool) -> Self {
        Self {
            pid,
            quiesced,
            prepared: AtomicBool::new(false),
            pending: AtomicBool::new(false),
            forker: AtomicU64::new(0),
            gate: Mutex::new(()),
            released: Condvar::new(),
            blocks: Mutex::new(Vec::new()),
            next: OnceLock::new(),
        }
    }

    /// Whether this process came from a fork its runners were idle for.
    pub const fn quiesced(&self) -> bool {
        self.quiesced
    }

    /// Whether `thread` is forking, so that waiting on a runner would wait on
    /// the fork that waits for it.
    pub fn forking(&self, thread: u64) -> bool {
        self.pending.load(Ordering::SeqCst) && self.forker.load(Ordering::SeqCst) == thread
    }

    pub fn register(&self, block: &Arc<ControlBlock>) {
        let mut blocks = lock(&self.blocks);
        blocks.retain(|block| block.strong_count() > 0);
        blocks.push(Arc::downgrade(block));
    }

    /// Called by a runner about to use its isolate: once a fork began, it
    /// waits without the isolate until the fork is over.
    pub fn resume(&self, block: &ControlBlock) {
        block.phase.set(Phase::Running);
        self.pause(block);
    }

    /// Waits out a pending fork; a runner interrupted in JavaScript holds its
    /// isolate meanwhile, and the child leaves that context alone.
    pub fn pause(&self, block: &ControlBlock) {
        if !self.pending.load(Ordering::SeqCst) {
            return;
        }
        block.phase.set(Phase::ParkedForFork);
        let mut gate = lock(&self.gate);
        while self.pending.load(Ordering::SeqCst) {
            gate = wait(&self.released, gate);
        }
        drop(gate);
        block.phase.set(Phase::Running);
    }
}

/// The generation of the calling process.
pub fn current() -> &'static Generation {
    let pid = std::process::id();
    let mut generation = ROOT.get_or_init(|| Generation::new(pid, false));
    while generation.pid != pid {
        let quiesced = generation.prepared.load(Ordering::SeqCst);
        generation = generation
            .next
            .get_or_init(|| Box::new(Generation::new(pid, quiesced)));
    }
    generation
}

/// Before a fork: with a single-threaded V8, waits until every runner is idle
/// or parked, so the child sees no isolate mid-change. Pair it with
/// `after_fork` in the parent.
pub fn prepare_fork(thread: u64) {
    let generation = current();
    generation.forker.store(thread, Ordering::SeqCst);
    if platform::mode() != Mode::SingleThreaded {
        return;
    }
    // Set before the registry is read: a runner registered later sees it.
    generation.pending.store(true, Ordering::SeqCst);
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
                if let Some(handle) = block.isolate.get() {
                    sys::interrupt_for_fork(handle);
                }
            }
        }
        std::thread::sleep(POLL);
    };
    generation.prepared.store(quiesced, Ordering::SeqCst);
}

/// In the parent once the fork is over, whether or not it succeeded.
pub fn after_fork() {
    let generation = current();
    generation.prepared.store(false, Ordering::SeqCst);
    generation.pending.store(false, Ordering::SeqCst);
    generation.forker.store(0, Ordering::SeqCst);
    let gate = lock(&generation.gate);
    generation.released.notify_all();
    drop(gate);
}
