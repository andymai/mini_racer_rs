use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use mini_racer_sync::{Channel, DispatchId, Protocol, RunnerPhase, TermCell, Terminator, lock};

use crate::notify::NotifyFd;
use crate::process::tasks::Tasks;
use crate::{EngineError, SnapshotBlob};

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
    /// The caller abandoned the dispatch: V8 throws `Error: terminated` and,
    /// unless the script already has its verdict, terminates it.
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
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum JsText {
    Utf8(Vec<u8>),
    Latin1(Vec<u8>),
    Utf16(Vec<u16>),
}

impl JsText {
    /// Its length in code units of its own encoding.
    #[must_use]
    pub const fn units(&self) -> usize {
        match self {
            Self::Utf8(bytes) | Self::Latin1(bytes) => bytes.len(),
            Self::Utf16(units) => units.len(),
        }
    }
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

/// A call in the wire form `mini_racer` sends: one array holding the
/// function's dotted path, then its arguments.
///
/// `objects` and `properties` count the objects the arguments hold and their
/// properties in all, which decide whether the runner builds the arguments.
pub struct CallRequest {
    pub bytes: Vec<u8>,
    pub objects: usize,
    pub properties: usize,
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
        request: CallRequest,
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
        base: Option<SnapshotBlob>,
        depth: Depth,
    },
    Dispose,
}

pub struct IsolateTerminator {
    handle: v8::IsolateHandle,
    tasks: Arc<Tasks>,
    requested: Arc<AtomicBool>,
}

impl IsolateTerminator {
    pub fn new(block: &ControlBlock, handle: v8::IsolateHandle) -> Self {
        Self {
            handle,
            tasks: Arc::clone(&block.tasks),
            requested: Arc::clone(&block.termination_requested),
        }
    }
}

impl Terminator for IsolateTerminator {
    // A runner awaiting a promise sleeps until a task is due; the ring wakes
    // it to find the termination.
    fn terminate(&self) {
        self.requested.store(true, Ordering::Release);
        self.handle.terminate_execution();
        self.tasks.ring();
    }

    fn cancel(&self) {
        self.handle.cancel_terminate_execution();
        self.requested.store(false, Ordering::Release);
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ContextConfig {
    pub timeout: Option<Duration>,
    /// The blob the isolate starts from, shared with every other context made
    /// from it. V8 reads it for as long as the isolate lives, so it stays here,
    /// in the block the runner holds until it has dropped the isolate.
    pub snapshot: Option<SnapshotBlob>,
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
    /// Spike: a hard heap cap from `CreateParams::heap_limits(0, limit)` and a
    /// near-heap-limit callback.
    pub heap_cap: Option<HeapCap>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Headroom {
    Bytes(usize),
    Percent(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HeapCap {
    pub limit: usize,
    pub headroom: Headroom,
    /// How many times one hit may raise the limit by the headroom before the
    /// callback refuses and V8 aborts.
    pub extensions: usize,
    /// Refuse every later script when a full collection after a hit leaves
    /// the heap over the cap, rather than restoring the limit anyway.
    pub poison: bool,
    pub log: bool,
}

impl HeapCap {
    /// Spike knobs, read by the extension at `Context.new`.
    #[must_use]
    pub fn from_env() -> Option<Self> {
        let limit = parse_size(&std::env::var("MINI_RACER_RS_HEAP_LIMIT").ok()?)?;
        let headroom = std::env::var("MINI_RACER_RS_HEAP_HEADROOM")
            .ok()
            .and_then(|text| {
                text.strip_suffix('%').map_or_else(
                    || parse_size(&text).map(Headroom::Bytes),
                    |percent| percent.parse().ok().map(Headroom::Percent),
                )
            })
            .unwrap_or(Headroom::Bytes(16 << 20));
        let extensions = std::env::var("MINI_RACER_RS_HEAP_EXTENSIONS")
            .ok()
            .and_then(|text| text.parse().ok())
            .unwrap_or(1);
        let poison = std::env::var_os("MINI_RACER_RS_HEAP_POISON").is_none_or(|v| v != "0");
        let log = std::env::var_os("MINI_RACER_RS_HEAP_LOG").is_some_and(|v| v == "1");
        Some(Self {
            limit,
            headroom,
            extensions,
            poison,
            log,
        })
    }

    #[must_use]
    pub const fn headroom_for(&self, initial: usize) -> usize {
        match self.headroom {
            Headroom::Bytes(bytes) => bytes,
            Headroom::Percent(percent) => initial / 100 * percent,
        }
    }
}

fn parse_size(text: &str) -> Option<usize> {
    let text = text.trim().to_ascii_lowercase();
    let (digits, shift) = match text.as_bytes().last()? {
        b'k' => (&text[..text.len() - 1], 10),
        b'm' => (&text[..text.len() - 1], 20),
        b'g' => (&text[..text.len() - 1], 30),
        _ => (text.as_str(), 0),
    };
    digits.parse::<usize>().ok().map(|n| n << shift)
}

/// Spike: what the near-heap-limit callback and the runner share.
#[derive(Default)]
pub struct CapState {
    /// Callback invocations over the context's life.
    pub calls: std::sync::atomic::AtomicUsize,
    /// Callback invocations since the last restore.
    pub episode: std::sync::atomic::AtomicUsize,
    /// The old generation limit V8 started the isolate with.
    pub initial: std::sync::atomic::AtomicUsize,
    /// `process::thread_token` of the runner, and of the callback's last call.
    pub runner_thread: std::sync::atomic::AtomicU64,
    pub callback_thread: std::sync::atomic::AtomicU64,
    pub off_thread_calls: std::sync::atomic::AtomicUsize,
    /// The runner is collecting after a hit, with no dispatch armed.
    pub collecting: AtomicBool,
    pub poisoned: AtomicBool,
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
    /// Set before anything asks V8 to terminate this context's script, and
    /// cleared only once V8 has cancelled every such request, so a runner
    /// building values itself knows when V8's own reader would stop instead.
    pub termination_requested: Arc<AtomicBool>,
    pub phase: RunnerPhase,
    pub isolate: OnceLock<v8::IsolateHandle>,
    // The isolate a snapshot is being built in on this runner, if any.
    pub building: Mutex<Option<v8::IsolateHandle>>,
    // An idle single-threaded runner leaves its isolate here, its only owner
    // then, so a forked child can hand it to a runner of its own.
    pub custody: Mutex<Option<v8::SharedIsolate>>,
    pub cap: CapState,
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
            #[expect(clippy::mem_forget, reason = "the leaked isolate may still read it")]
            std::mem::forget(self.config.snapshot.take());
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
            termination_requested: Arc::default(),
            phase: RunnerPhase::default(),
            isolate: OnceLock::new(),
            building: Mutex::new(None),
            custody: Mutex::new(None),
            cap: CapState::default(),
        }
    }
}
