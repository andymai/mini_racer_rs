use std::cell::Cell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
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

// One process of a fork lineage. A child makes its own on first use and never
// locks or frees what it inherited, which a thread that is gone may hold.
pub struct Generation {
    pid: u32,
    // Whether every fork that led to this process found its runners idle: a
    // runner caught mid-change by any of them may have left V8's process-wide
    // state locked for every process after it. True for the first process.
    quiesced: bool,
    prepared: AtomicBool,
    // Whether this process's pending fork shut the gate.
    begun: AtomicBool,
    gate: ForkGate,
    blocks: Mutex<Vec<Weak<ControlBlock>>>,
    next: OnceLock<Box<Self>>,
}

static ROOT: OnceLock<Generation> = OnceLock::new();

impl Generation {
    fn new(pid: u32, quiesced: bool) -> Self {
        Self {
            pid,
            quiesced,
            prepared: AtomicBool::new(false),
            begun: AtomicBool::new(false),
            gate: ForkGate::default(),
            blocks: Mutex::new(Vec::new()),
            next: OnceLock::new(),
        }
    }

    /// Whether every fork that led to this process found its runners idle.
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
    let pid = std::process::id();
    let mut generation = ROOT.get_or_init(|| Generation::new(pid, true));
    while generation.pid != pid {
        let quiesced = generation.quiesced && generation.prepared.load(Ordering::SeqCst);
        generation = generation
            .next
            .get_or_init(|| Box::new(Generation::new(pid, quiesced)));
    }
    generation
}

thread_local! {
    // Set while a fork from Ruby runs on this thread. Forks that run no Ruby
    // in the child (spawn, system and backticks on macOS) leave runners be.
    static INTENDED: Cell<bool> = const { Cell::new(false) };
}

/// Marks the calling thread's forks, until cleared, as ones whose child runs
/// Ruby and may use the contexts it inherits.
pub fn intend(forking: bool) {
    INTENDED.with(|intended| intended.set(forking));
}

// pthread_atfork's prepare handler, on the forking thread inside fork() and
// after every Ruby `_fork` hook, so no Ruby code runs while runners are held.
// With a single-threaded V8 it waits until every runner is idle or parked, so
// the child sees no isolate mid-change. No runner needs Ruby to get there:
// one waiting on a callback already counts as parked.
pub(crate) fn prepare() {
    if !INTENDED.with(Cell::get) || platform::mode() != Mode::SingleThreaded {
        return;
    }
    let generation = current();
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
    generation.begun.store(true, Ordering::SeqCst);
}

// pthread_atfork's parent handler, whether or not the fork succeeded. The
// child needs none: it reads what it inherited from `prepared`.
pub(crate) fn parent() {
    let generation = current();
    if generation.begun.swap(false, Ordering::SeqCst) {
        generation.prepared.store(false, Ordering::SeqCst);
        generation.gate.end();
    }
}
