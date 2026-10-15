use std::sync::mpsc::{self, Sender, SyncSender};

use crate::{EngineError, process, runner};

const RUNNER_STACK_BYTES: usize = 2 << 20;

pub enum Request {
    Eval {
        source: String,
        reply: SyncSender<Result<Vec<u8>, EngineError>>,
    },
}

pub struct ContextHandle {
    requests: Sender<Request>,
}

impl ContextHandle {
    pub fn spawn() -> Result<Self, EngineError> {
        process::ensure_initialized();
        let (requests, inbox) = mpsc::channel();
        std::thread::Builder::new()
            .name("mini_racer_rs".to_owned())
            .stack_size(RUNNER_STACK_BYTES)
            .spawn(move || runner::run(&inbox))
            .map_err(|e| EngineError::Internal(format!("spawn runner: {e}")))?;
        Ok(Self { requests })
    }

    pub fn eval(&self, source: String) -> Result<Vec<u8>, EngineError> {
        let (reply, response) = mpsc::sync_channel(1);
        self.requests
            .send(Request::Eval { source, reply })
            .map_err(|_| EngineError::Disposed)?;
        response
            .recv()
            .map_err(|_| EngineError::Internal("runner exited".to_owned()))?
    }
}
