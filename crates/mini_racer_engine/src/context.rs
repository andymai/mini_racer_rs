use std::sync::{Arc, Mutex};
use std::time::Duration;

use mini_racer_sync::{
    Channel, ChannelError, DispatchId, DispatchKind, Interrupt, Side, TermCell, TermError, ToRuby,
};

use crate::control::{ContextConfig, ControlBlock, Request, Wire};
use crate::process::{self, platform, watchdog};
use crate::spin::spin_until;
use crate::{EngineError, runner};

const MIN_RUNNER_STACK_BYTES: usize = 2 << 20;
const STACK_HEADROOM_BYTES: usize = 1 << 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitOutcome {
    Ready,
    /// Abandon the dispatch: the engine terminates it and then waits for its
    /// response through `Host::wait_blocking`.
    Cancel,
}

/// How the calling thread waits for its runner. The extension spins, releases
/// the GVL and parks, and turns Ruby interrupts into `Cancel`.
pub trait Host {
    /// Returns `Ready` once `wait.ready()` holds, or `Cancel`.
    fn wait(&mut self, wait: &WaitHandle<'_>) -> WaitOutcome;
    /// Returns once `wait.ready()` holds, whatever interrupts arrive.
    fn wait_blocking(&mut self, wait: &WaitHandle<'_>);
}

pub struct WaitHandle<'a> {
    channel: &'a Channel<Wire>,
}

impl<'a> WaitHandle<'a> {
    #[must_use]
    pub fn ready(&self) -> bool {
        self.channel.ready(Side::Ruby)
    }

    /// Spins up to `budget`; returns whether the runner answered meanwhile.
    #[must_use]
    pub fn spin(&self, budget: Duration) -> bool {
        spin_until(budget, || self.ready())
    }

    /// Sleeps until the runner answers or `Unparker::unpark`; may return early.
    pub fn park(&self) {
        self.channel.park(Side::Ruby, None);
    }

    #[must_use]
    pub const fn unparker(&self) -> Unparker<'a> {
        Unparker {
            channel: self.channel,
        }
    }
}

/// The only thing an interrupt handler on another thread may touch.
#[derive(Clone, Copy)]
pub struct Unparker<'a> {
    channel: &'a Channel<Wire>,
}

impl Unparker<'_> {
    pub fn unpark(self) {
        self.channel.wake(Side::Ruby);
    }
}

pub struct ContextHandle {
    block: Arc<ControlBlock>,
    pid: u32,
}

impl ContextHandle {
    /// Starts a runner and waits, through `host`, until its isolate exists.
    pub fn spawn(config: ContextConfig, host: &mut impl Host) -> Result<Self, EngineError> {
        process::ensure_initialized();
        let block = Arc::new(ControlBlock {
            channel: Channel::default(),
            term: TermCell::default(),
            config,
            timer: Mutex::default(),
        });
        let runner_block = Arc::clone(&block);
        let stack = platform::stack_size_kb().map_or(MIN_RUNNER_STACK_BYTES, |kb| {
            (kb.saturating_mul(1024).saturating_add(STACK_HEADROOM_BYTES))
                .max(MIN_RUNNER_STACK_BYTES)
        });
        std::thread::Builder::new()
            .name("mini_racer_rs".to_owned())
            .stack_size(stack)
            .spawn(move || runner::run(&runner_block))
            .map_err(|e| EngineError::Internal(format!("spawn runner: {e}")))?;
        let handle = Self {
            block,
            pid: std::process::id(),
        };
        handle.dispatch(
            DispatchKind::Maintenance,
            |dispatch| Request::Init { dispatch },
            host,
        )?;
        Ok(handle)
    }

    pub fn eval(&self, source: String, host: &mut impl Host) -> Result<Vec<u8>, EngineError> {
        self.dispatch(
            DispatchKind::Script,
            |dispatch| Request::Eval { source, dispatch },
            host,
        )
    }

    /// Terminates the running script, or the next one if none is running.
    pub fn stop(&self) -> Result<(), EngineError> {
        if self.orphaned() {
            return Ok(());
        }
        if self.is_disposed() {
            return Err(EngineError::Disposed);
        }
        self.block.term.request(Interrupt::Stop);
        Ok(())
    }

    /// Refuses new dispatches and terminates one in flight on another thread.
    /// The caller then waits for that dispatch to finish before `dispose`.
    pub fn begin_dispose(&self) {
        if !self.orphaned() {
            self.block.term.dispose();
        }
    }

    /// Shuts the runner down and returns once its isolate is gone. Expects
    /// `begin_dispose` and no dispatch in flight; a second call is a no-op.
    pub fn dispose(&self, host: &mut impl Host) -> Result<(), EngineError> {
        // mini_racer returns quietly for an idle context a forked child inherits.
        if self.orphaned() {
            return Ok(());
        }
        self.block.term.dispose();
        match self.block.channel.request(Request::Dispose) {
            Ok(()) => {}
            Err(ChannelError::Closed) => return Ok(()),
            Err(error) => return Err(protocol(error)),
        }
        let wait = WaitHandle {
            channel: &self.block.channel,
        };
        loop {
            host.wait_blocking(&wait);
            match self.block.channel.recv_ruby() {
                Ok(Some(ToRuby::Response(_))) | Err(ChannelError::Closed) => return Ok(()),
                Ok(Some(ToRuby::Callback(callback))) => match callback {},
                Ok(None) => {}
                Err(error) => return Err(protocol(error)),
            }
        }
    }

    #[must_use]
    pub fn is_disposed(&self) -> bool {
        self.block.channel.is_closed() || self.block.term.is_disposing()
    }

    // A forked child inherits this handle but not the runner thread behind it.
    fn orphaned(&self) -> bool {
        std::process::id() != self.pid
    }

    fn dispatch(
        &self,
        kind: DispatchKind,
        request: impl FnOnce(DispatchId) -> Request,
        host: &mut impl Host,
    ) -> Result<Vec<u8>, EngineError> {
        if self.orphaned() {
            return Err(EngineError::Orphaned);
        }
        let block = &self.block;
        let dispatch = block.term.begin(kind).map_err(|error| match error {
            TermError::Disposing => EngineError::Disposed,
            TermError::AlreadyArmed => EngineError::Busy,
        })?;
        // mini_racer times only scripts, never setup or maintenance.
        if let (DispatchKind::Script, Some(timeout)) = (kind, block.config.timeout) {
            watchdog::schedule(timeout, block, dispatch);
        }
        if let Err(error) = block.channel.request(request(dispatch)) {
            block.term.end(dispatch);
            return Err(match error {
                ChannelError::Closed => EngineError::Disposed,
                error => protocol(error),
            });
        }
        let wait = WaitHandle {
            channel: &block.channel,
        };
        let mut cancelled = false;
        loop {
            if cancelled {
                host.wait_blocking(&wait);
            } else if host.wait(&wait) == WaitOutcome::Cancel {
                cancelled = true;
                block.term.request(Interrupt::Cancel(dispatch));
                continue;
            }
            match block.channel.recv_ruby() {
                Ok(Some(ToRuby::Response(response))) => {
                    // A no-op unless the runner died before disarming it.
                    block.term.end(dispatch);
                    watchdog::clear(block, dispatch);
                    return response;
                }
                Ok(Some(ToRuby::Callback(callback))) => match callback {},
                Ok(None) => {}
                Err(error) => {
                    block.term.end(dispatch);
                    watchdog::clear(block, dispatch);
                    return Err(match error {
                        ChannelError::Closed => EngineError::Disposed,
                        error => protocol(error),
                    });
                }
            }
        }
    }
}

impl Drop for ContextHandle {
    // A finalizer never blocks: the runner sees the closed channel, drops its
    // isolate on its own thread and exits.
    // An inherited handle is never touched: its locks may have been held by
    // parent threads that do not exist in this process.
    fn drop(&mut self) {
        if self.orphaned() {
            return;
        }
        self.block.term.dispose();
        self.block.channel.close();
    }
}

fn protocol(error: ChannelError) -> EngineError {
    EngineError::Internal(format!("channel protocol: {error}"))
}
