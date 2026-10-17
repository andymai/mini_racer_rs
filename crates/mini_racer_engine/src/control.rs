use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use mini_racer_sync::{Channel, DispatchId, Protocol, RunnerPhase, TermCell, Terminator, lock};

use crate::EngineError;
use crate::notify::NotifyFd;
use crate::process::tasks::Tasks;

pub struct Wire;

impl Protocol for Wire {
    type Request = Request;
    type Response = Result<Vec<u8>, EngineError>;
    type Callback = Vec<u8>;
    type Reply = CallbackReply;
}

pub enum CallbackReply {
    Value(Vec<u8>),
    /// The callback raised: V8 throws an `Error` with this message, and an
    /// outermost or nested dispatch it escapes ends with
    /// `JsErrorKind::HostException`.
    Raised(JsText),
    /// The caller abandoned the dispatch: V8 throws `Error: terminated` and
    /// terminates the script.
    Abort,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Depth {
    Outermost(DispatchId),
    Nested,
}

/// Text in the encoding Ruby holds it in. V8 reads each form directly, so a
/// lone surrogate in UTF-16 and invalid UTF-8 reach it as `mini_racer` sends
/// them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JsText {
    Utf8(Vec<u8>),
    Latin1(Vec<u8>),
    Utf16(Vec<u16>),
}

impl From<String> for JsText {
    fn from(text: String) -> Self {
        Self::Utf8(text.into_bytes())
    }
}

impl From<&str> for JsText {
    fn from(text: &str) -> Self {
        Self::Utf8(text.as_bytes().to_vec())
    }
}

pub enum Request {
    Init {
        depth: Depth,
    },
    /// `awaited`: a promise result is awaited, as `eval_await` does.
    Eval {
        filename: JsText,
        source: JsText,
        depth: Depth,
        awaited: bool,
    },
    Call {
        request: Vec<u8>,
        depth: Depth,
        awaited: bool,
    },
    Attach {
        request: Vec<u8>,
        depth: Depth,
    },
    LowMemoryNotification {
        depth: Depth,
    },
    MicrotaskCheckpoint {
        depth: Depth,
    },
    /// The response is a wire object of numbers, under `mini_racer`'s names.
    HeapStats {
        depth: Depth,
    },
    /// The response is the JSON V8 writes, not wire bytes.
    HeapSnapshot {
        depth: Depth,
    },
    /// Runs one due task; the response is one byte, 1 if a task ran.
    PumpMessageLoop {
        depth: Depth,
    },
    // Built on the runner of a context made for it, as mini_racer builds one
    // on a throwaway context: from `code` alone, or by warming `base` up.
    Snapshot {
        code: JsText,
        base: Option<Vec<u8>>,
        depth: Depth,
    },
    Dispose,
}

pub struct IsolateTerminator {
    handle: v8::IsolateHandle,
    tasks: Arc<Tasks>,
}

impl IsolateTerminator {
    pub fn new(handle: v8::IsolateHandle, tasks: &Arc<Tasks>) -> Self {
        Self {
            handle,
            tasks: Arc::clone(tasks),
        }
    }
}

impl Terminator for IsolateTerminator {
    // A runner awaiting a promise sleeps until a task is due; the ring wakes
    // it to find the termination.
    fn terminate(&self) {
        self.handle.terminate_execution();
        self.tasks.ring();
    }

    fn cancel(&self) {
        self.handle.cancel_terminate_execution();
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ContextConfig {
    pub timeout: Option<Duration>,
    /// A blob from `ContextHandle::create_snapshot` the isolate starts from;
    /// the runner takes it, so V8 holds the only copy.
    pub snapshot: Option<Vec<u8>>,
    /// The used heap size, in bytes, past which a garbage collection ends the
    /// running script out of memory, as `mini_racer`'s `max_memory:` does.
    pub max_memory: Option<u64>,
    /// How long a runner waits idle after a request before it asks V8 for a
    /// full garbage collection, once, as `mini_racer`'s
    /// `ensure_gc_after_idle:` does. Single-threaded runners never do.
    pub gc_after_idle: Option<Duration>,
    /// Has V8 report each exception that escapes a script, as `mini_racer`'s
    /// verbose `TryCatch` does: it prints where it happened to stdout.
    pub verbose_exceptions: bool,
}

/// The context's pending timeout and the earliest deadline the watchdog holds
/// for it, so the watchdog keeps about one entry per context, not per dispatch.
#[derive(Default)]
pub struct Timer {
    pub due: Option<(Instant, DispatchId)>,
    pub queued: Option<Instant>,
}

// Shared by the handle, its runner and the watchdog's weak references.
pub struct ControlBlock {
    pub channel: Channel<Wire>,
    pub term: TermCell<IsolateTerminator>,
    pub config: ContextConfig,
    pub timer: Mutex<Timer>,
    pub notify: NotifyFd,
    // V8's foreground tasks for this context's isolate, which the runner runs
    // while awaiting a promise or pumping.
    pub tasks: Arc<Tasks>,
    pub phase: RunnerPhase,
    pub isolate: OnceLock<v8::IsolateHandle>,
    // The isolate a snapshot is being built in on this runner, if any.
    pub building: Mutex<Option<v8::IsolateHandle>>,
    // An idle single-threaded runner leaves its isolate here, its only owner
    // then, so a forked child can hand it to a runner of its own.
    pub custody: Mutex<Option<v8::SharedIsolate>>,
}

impl Drop for ControlBlock {
    // An isolate is dropped only by a runner (I8). One still here was never
    // taken back, as when its runner thread could not start or panicked.
    fn drop(&mut self) {
        let isolate = lock(&self.custody).take();
        if let Some(isolate) = isolate {
            #[expect(
                clippy::mem_forget,
                reason = "leaked rather than disposed off its runner"
            )]
            std::mem::forget(isolate);
        }
    }
}

impl ControlBlock {
    /// Asks the runner to park for a pending fork, in JavaScript or in a
    /// snapshot build, wherever V8 next checks for interrupts.
    pub fn interrupt_for_fork(&self) {
        if let Some(handle) = self.isolate.get() {
            crate::sys::interrupt_for_fork(handle);
        }
        let building = lock(&self.building).clone();
        if let Some(handle) = building {
            crate::sys::interrupt_for_fork(&handle);
        }
    }

    pub fn new(config: ContextConfig) -> Self {
        Self::with_tasks(config, Tasks::default())
    }

    /// For a forked child taking over an idle context with its queued tasks.
    pub fn with_tasks(config: ContextConfig, tasks: Tasks) -> Self {
        Self {
            channel: Channel::default(),
            term: TermCell::default(),
            config,
            timer: Mutex::default(),
            notify: NotifyFd::default(),
            tasks: Arc::new(tasks),
            phase: RunnerPhase::default(),
            isolate: OnceLock::new(),
            building: Mutex::new(None),
            custody: Mutex::new(None),
        }
    }
}
