use std::sync::Mutex;
use std::time::{Duration, Instant};

use mini_racer_sync::{Channel, DispatchId, Protocol, TermCell, Terminator};

use crate::EngineError;

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
    Eval {
        filename: JsText,
        source: JsText,
        depth: Depth,
    },
    Call {
        request: Vec<u8>,
        depth: Depth,
    },
    Attach {
        request: Vec<u8>,
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

pub struct IsolateTerminator(pub v8::IsolateHandle);

impl Terminator for IsolateTerminator {
    fn terminate(&self) {
        self.0.terminate_execution();
    }

    fn cancel(&self) {
        self.0.cancel_terminate_execution();
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ContextConfig {
    pub timeout: Option<Duration>,
    /// A blob from `ContextHandle::create_snapshot` the isolate starts from;
    /// the runner takes it, so V8 holds the only copy.
    pub snapshot: Option<Vec<u8>>,
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
}
