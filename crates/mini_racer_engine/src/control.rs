use std::convert::Infallible;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use mini_racer_sync::{Channel, DispatchId, Protocol, TermCell, Terminator};

use crate::EngineError;

pub struct Wire;

impl Protocol for Wire {
    type Request = Request;
    type Response = Result<Vec<u8>, EngineError>;
    type Callback = Infallible;
    type Reply = Infallible;
}

pub enum Request {
    Init {
        dispatch: DispatchId,
    },
    Eval {
        filename: String,
        source: String,
        dispatch: DispatchId,
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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ContextConfig {
    pub timeout: Option<Duration>,
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
