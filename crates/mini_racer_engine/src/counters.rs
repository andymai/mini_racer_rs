use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use crate::control::CallbackReply;
use crate::{EngineError, JsError, JsErrorKind, TerminationCause};

/// What a context has done since it was made; a forked child that takes it
/// over counts on from its parent's counts.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counts {
    /// Requests sent to the runner, nested ones included: evals, calls and
    /// their await forms, attaches, heap statistics and snapshots, low memory
    /// notifications, microtask checkpoints and pumps; not the context's own
    /// setup or dispose.
    pub dispatches: u64,
    /// Attached Ruby callables run for JavaScript.
    pub callbacks: u64,
    /// An eval's source in the encoding it was sent in, a call's or attach's
    /// request and a callback's value in V8's wire format, and the message of
    /// an error a callback raised.
    pub bytes_to_js: u64,
    /// Each result the runner answered with and each callback's arguments.
    pub bytes_from_js: u64,
    /// The runner's time on evals, calls, attaches, microtask checkpoints and
    /// pumps, short of its waits for Ruby callbacks, for a task an await
    /// needs and for a fork.
    pub js_ns: u64,
    /// V8's scavenges, minor mark-sweeps and mark-compacts on the context's
    /// isolate, in a dispatch or not.
    pub gc_count: u64,
    /// The runner's time inside them, from V8's prologue to its epilogue.
    pub gc_ns: u64,
    pub terminated: Terminations,
}

/// Dispatches a termination ended, by the cause the engine reported for each.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Terminations {
    pub stop: u64,
    pub timeout: u64,
    /// The caller abandoned the wait: an exception raised into it, or a throw
    /// or kill out of a callback.
    pub interrupt: u64,
    pub max_memory: u64,
    pub heap_exhausted: u64,
}

/// A context's counts, in the control block. Each half has one writer at a
/// time, so an increment is a load and a store rather than a locked add: the
/// thread dispatching, which holds the context, or the runner. Readers on any
/// thread see each value on its own, never torn.
#[derive(Default)]
pub struct Counters {
    caller: Caller,
    runner: Runner,
}

// 128 bytes apart, as x86's prefetcher fetches 64-byte lines in pairs and
// Apple's cores have 128-byte lines, so the two writers never share one.
#[derive(Default)]
#[repr(align(128))]
struct Caller {
    dispatches: AtomicU64,
    callbacks: AtomicU64,
    bytes_to_js: AtomicU64,
    bytes_from_js: AtomicU64,
    terminated: [AtomicU64; 5],
}

#[derive(Default)]
#[repr(align(128))]
struct Runner {
    js_ns: AtomicU64,
    gc_count: AtomicU64,
    gc_ns: AtomicU64,
}

fn add(counter: &AtomicU64, amount: u64) {
    counter.store(
        counter.load(Ordering::Relaxed).wrapping_add(amount),
        Ordering::Relaxed,
    );
}

fn read(counter: &AtomicU64) -> u64 {
    counter.load(Ordering::Relaxed)
}

fn copied(counter: &AtomicU64) -> AtomicU64 {
    AtomicU64::new(read(counter))
}

fn length(bytes: usize) -> u64 {
    u64::try_from(bytes).unwrap_or(u64::MAX)
}

const fn slot(cause: TerminationCause) -> usize {
    match cause {
        TerminationCause::Stop => 0,
        TerminationCause::Timeout => 1,
        TerminationCause::Cancelled => 2,
        TerminationCause::MaxMemory => 3,
        TerminationCause::HeapExhausted => 4,
    }
}

impl Counters {
    #[must_use]
    pub fn read(&self) -> Counts {
        let caller = &self.caller;
        let terminated = |cause| read(&caller.terminated[slot(cause)]);
        Counts {
            dispatches: read(&caller.dispatches),
            callbacks: read(&caller.callbacks),
            bytes_to_js: read(&caller.bytes_to_js),
            bytes_from_js: read(&caller.bytes_from_js),
            js_ns: read(&self.runner.js_ns),
            gc_count: read(&self.runner.gc_count),
            gc_ns: read(&self.runner.gc_ns),
            terminated: Terminations {
                stop: terminated(TerminationCause::Stop),
                timeout: terminated(TerminationCause::Timeout),
                interrupt: terminated(TerminationCause::Cancelled),
                max_memory: terminated(TerminationCause::MaxMemory),
                heap_exhausted: terminated(TerminationCause::HeapExhausted),
            },
        }
    }

    /// The counts so far, for a forked child taking the context over, read
    /// from atomics alone.
    #[must_use]
    pub fn inherited(&self) -> Self {
        let caller = &self.caller;
        Self {
            caller: Caller {
                dispatches: copied(&caller.dispatches),
                callbacks: copied(&caller.callbacks),
                bytes_to_js: copied(&caller.bytes_to_js),
                bytes_from_js: copied(&caller.bytes_from_js),
                terminated: std::array::from_fn(|at| copied(&caller.terminated[at])),
            },
            runner: Runner {
                js_ns: copied(&self.runner.js_ns),
                gc_count: copied(&self.runner.gc_count),
                gc_ns: copied(&self.runner.gc_ns),
            },
        }
    }

    pub(crate) fn dispatched(&self, bytes: usize) {
        add(&self.caller.dispatches, 1);
        add(&self.caller.bytes_to_js, length(bytes));
    }

    pub(crate) fn asked(&self, bytes: usize) {
        add(&self.caller.bytes_from_js, length(bytes));
    }

    pub(crate) fn called_back(&self) {
        add(&self.caller.callbacks, 1);
    }

    pub(crate) fn replied(&self, reply: &CallbackReply) {
        let bytes = match reply {
            CallbackReply::Value(bytes) => bytes.len(),
            CallbackReply::Raised(message) => message.byte_len(),
            CallbackReply::Abort => return,
        };
        add(&self.caller.bytes_to_js, length(bytes));
    }

    /// A dispatch's response as its caller gets it. A nested dispatch its
    /// caller abandoned was ended without a cause of its own
    /// (`TermCell::interrupt`), so its termination is the interrupt's, which
    /// outranks every cause but the memory ones, as the engine ranks them.
    pub(crate) fn answered(&self, response: &Result<Vec<u8>, EngineError>, abandoned: bool) {
        match response {
            Ok(bytes) => add(&self.caller.bytes_from_js, length(bytes.len())),
            Err(EngineError::Js(JsError {
                kind: JsErrorKind::Terminated(cause),
                ..
            })) => {
                let cause = match *cause {
                    memory @ (TerminationCause::MaxMemory | TerminationCause::HeapExhausted) => {
                        memory
                    }
                    _ if abandoned => TerminationCause::Cancelled,
                    other => other,
                };
                add(&self.caller.terminated[slot(cause)], 1);
            }
            Err(_) => {}
        }
    }

    pub(crate) fn ran_js(&self, nanoseconds: u64) {
        add(&self.runner.js_ns, nanoseconds);
    }

    pub(crate) fn collected(&self, took: Duration) {
        add(&self.runner.gc_count, 1);
        add(
            &self.runner.gc_ns,
            u64::try_from(took.as_nanos()).unwrap_or(u64::MAX),
        );
    }
}
