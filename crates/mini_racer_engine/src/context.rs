use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use mini_racer_sync::{Handoff, Outcome};

use crate::process::{self, platform};
use crate::{EngineError, runner};

const MIN_RUNNER_STACK_BYTES: usize = 2 << 20;
const STACK_HEADROOM_BYTES: usize = 1 << 20;

type EvalResult = Result<Vec<u8>, EngineError>;

pub enum Request {
    Eval {
        source: String,
        slot: Arc<Handoff<EvalResult>>,
    },
}

pub enum WaitOutcome {
    Done(EvalResult),
    Interrupted,
}

pub struct Pending {
    slot: Arc<Handoff<EvalResult>>,
}

impl Pending {
    #[must_use]
    pub fn wait(&self) -> WaitOutcome {
        match self.slot.wait() {
            Outcome::Done(result) => WaitOutcome::Done(result),
            Outcome::Interrupted => WaitOutcome::Interrupted,
        }
    }

    #[must_use]
    pub fn wait_for(&self, timeout: Duration) -> Option<EvalResult> {
        self.slot.wait_for(timeout)
    }

    pub fn interrupt(&self) {
        self.slot.interrupt();
    }
}

pub struct ContextHandle {
    requests: Sender<Request>,
    isolate: Arc<OnceLock<v8::IsolateHandle>>,
    pid: u32,
}

impl ContextHandle {
    pub fn spawn() -> Result<Self, EngineError> {
        process::ensure_initialized();
        let (requests, inbox) = mpsc::channel();
        let isolate = Arc::new(OnceLock::new());
        let runner_isolate = Arc::clone(&isolate);
        let stack = platform::stack_size_kb().map_or(MIN_RUNNER_STACK_BYTES, |kb| {
            (kb.saturating_mul(1024).saturating_add(STACK_HEADROOM_BYTES))
                .max(MIN_RUNNER_STACK_BYTES)
        });
        std::thread::Builder::new()
            .name("mini_racer_rs".to_owned())
            .stack_size(stack)
            .spawn(move || runner::run(&inbox, &runner_isolate))
            .map_err(|e| EngineError::Internal(format!("spawn runner: {e}")))?;
        Ok(Self {
            requests,
            isolate,
            pid: std::process::id(),
        })
    }

    // A forked child inherits this handle but not the runner thread behind it.
    pub fn eval(&self, source: String) -> Result<Pending, EngineError> {
        if std::process::id() != self.pid {
            return Err(EngineError::Orphaned);
        }
        let slot = Arc::new(Handoff::default());
        self.requests
            .send(Request::Eval {
                source,
                slot: Arc::clone(&slot),
            })
            .map_err(|_| EngineError::Disposed)?;
        Ok(Pending { slot })
    }

    pub fn terminate(&self, pending: &Pending) {
        pending.slot.cancel();
        if let Some(isolate) = self.isolate.get() {
            isolate.terminate_execution();
        }
    }
}
