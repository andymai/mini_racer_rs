use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::Duration;

use crate::process::{self, platform};
use crate::{EngineError, runner};

const MIN_RUNNER_STACK_BYTES: usize = 2 << 20;
const STACK_HEADROOM_BYTES: usize = 1 << 20;

pub enum Request {
    Eval { source: String, slot: Arc<Slot> },
}

pub enum WaitOutcome {
    Done(Result<Vec<u8>, EngineError>),
    Interrupted,
}

#[derive(Default)]
struct SlotState {
    result: Option<Result<Vec<u8>, EngineError>>,
    interrupted: bool,
}

#[derive(Default)]
pub struct Slot {
    state: Mutex<SlotState>,
    changed: Condvar,
    cancelled: AtomicBool,
}

impl Slot {
    fn lock(&self) -> MutexGuard<'_, SlotState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    pub fn complete(&self, result: Result<Vec<u8>, EngineError>) {
        self.lock().result = Some(result);
        self.changed.notify_all();
    }
}

pub struct Pending {
    slot: Arc<Slot>,
}

impl Pending {
    pub fn wait(&self) -> WaitOutcome {
        let mut state = self.slot.lock();
        loop {
            if let Some(result) = state.result.take() {
                return WaitOutcome::Done(result);
            }
            if std::mem::take(&mut state.interrupted) {
                return WaitOutcome::Interrupted;
            }
            state = self
                .slot
                .changed
                .wait(state)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }

    pub fn wait_for(&self, timeout: Duration) -> Option<Result<Vec<u8>, EngineError>> {
        let (mut state, _) = self
            .slot
            .changed
            .wait_timeout_while(self.slot.lock(), timeout, |s| s.result.is_none())
            .unwrap_or_else(PoisonError::into_inner);
        let result = state.result.take();
        drop(state);
        result
    }

    pub fn interrupt(&self) {
        self.slot.lock().interrupted = true;
        self.slot.changed.notify_all();
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
        let slot = Arc::new(Slot::default());
        self.requests
            .send(Request::Eval {
                source,
                slot: Arc::clone(&slot),
            })
            .map_err(|_| EngineError::Disposed)?;
        Ok(Pending { slot })
    }

    pub fn terminate(&self, pending: &Pending) {
        pending.slot.cancelled.store(true, Ordering::SeqCst);
        if let Some(isolate) = self.isolate.get() {
            isolate.terminate_execution();
        }
    }
}
