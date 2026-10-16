use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::{Duration, Instant};

use mini_racer_sync::{ForkGate, Phase, lock};

use crate::control::ControlBlock;
use crate::process::platform::{self, Mode};
use crate::sys;

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
    // Whether this process came from a fork its runners were idle for: the
    // parent's `prepared`, as the child inherited it at the fork.
    quiesced: bool,
    prepared: AtomicBool,
    gate: ForkGate,
    // The thread whose fork is pending, which must not wait on a runner.
    forker: AtomicU64,
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
            gate: ForkGate::default(),
            forker: AtomicU64::new(0),
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
        self.forker.load(Ordering::SeqCst) == thread
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
    if platform::mode() != Mode::SingleThreaded {
        return;
    }
    let generation = current();
    generation.forker.store(thread, Ordering::SeqCst);
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
    generation.forker.store(0, Ordering::SeqCst);
    generation.gate.end();
}
