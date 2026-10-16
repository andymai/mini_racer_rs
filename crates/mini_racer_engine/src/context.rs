use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mini_racer_sync::{
    Channel, ChannelError, DispatchKind, Interrupt, Side, TermCell, TermError, ToRuby,
};

use crate::control::{CallbackReply, ContextConfig, ControlBlock, Depth, JsText, Request, Wire};
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
    /// Runs the callback `request` names: an array of its arguments with its
    /// id last, in V8's wire format. Dispatches it makes on this context from
    /// this thread run nested inside the current one. `Abort` cancels the
    /// dispatch.
    fn callback(&mut self, request: &[u8]) -> CallbackReply;
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
    session: Session,
}

// The thread running the outermost dispatch, the callbacks it is running and
// the nested dispatches those callbacks wait on. Only that thread may dispatch
// meanwhile, and only from a callback that is not already waiting on one: a
// signal trap during the wait would otherwise send out of turn.
#[derive(Default)]
struct Session {
    owner: AtomicU64,
    callbacks: AtomicU32,
    waits: AtomicU32,
}

impl Session {
    fn in_callback(&self, thread: u64) -> bool {
        self.owner.load(Ordering::Acquire) == thread
            && self.callbacks.load(Ordering::Relaxed) > self.waits.load(Ordering::Relaxed)
    }
}

struct Counted<'a>(&'a AtomicU32);

impl<'a> Counted<'a> {
    fn enter(count: &'a AtomicU32) -> Self {
        count.fetch_add(1, Ordering::Relaxed);
        Self(count)
    }
}

impl Drop for Counted<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

struct Owning<'a>(&'a AtomicU64);

impl Drop for Owning<'_> {
    fn drop(&mut self) {
        self.0.store(0, Ordering::Release);
    }
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
            session: Session::default(),
        };
        handle.dispatch(
            DispatchKind::Maintenance,
            |depth| Request::Init { depth },
            host,
        )?;
        Ok(handle)
    }

    pub fn eval(
        &self,
        filename: impl Into<JsText>,
        source: impl Into<JsText>,
        host: &mut impl Host,
    ) -> Result<Vec<u8>, EngineError> {
        let (filename, source) = (filename.into(), source.into());
        self.dispatch(
            DispatchKind::Script,
            |depth| Request::Eval {
                filename,
                source,
                depth,
            },
            host,
        )
    }

    /// `request` is the wire form `mini_racer` sends: one array holding the
    /// function's dotted path, then its arguments.
    pub fn call(&self, request: Vec<u8>, host: &mut impl Host) -> Result<Vec<u8>, EngineError> {
        self.dispatch(
            DispatchKind::Script,
            |depth| Request::Call { request, depth },
            host,
        )
    }

    /// `request` is the wire form `mini_racer` sends: an array of the dotted
    /// path to define and the callback id `Host::callback` will be given.
    pub fn attach(&self, request: Vec<u8>, host: &mut impl Host) -> Result<(), EngineError> {
        self.dispatch(
            DispatchKind::Maintenance,
            |depth| Request::Attach { request, depth },
            host,
        )
        .map(drop)
    }

    /// Whether this thread is running one of this context's callbacks, where a
    /// dispatch runs nested in the one that made the callback.
    #[must_use]
    pub fn in_callback(&self) -> bool {
        self.session.in_callback(process::thread_token())
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
        if self.session.callbacks.load(Ordering::Relaxed) > 0 {
            return Err(EngineError::DisposeBusy);
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
                Ok(Some(ToRuby::Response(response))) => return response.map(drop),
                Err(ChannelError::Closed) => return Ok(()),
                Ok(Some(ToRuby::Callback(_))) => {
                    let _ = self.block.channel.reply(CallbackReply::Abort);
                }
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
        request: impl FnOnce(Depth) -> Request,
        host: &mut impl Host,
    ) -> Result<Vec<u8>, EngineError> {
        if self.orphaned() {
            return Err(EngineError::Orphaned);
        }
        let thread = process::thread_token();
        match self.session.owner.load(Ordering::Acquire) {
            0 => {}
            _ if self.session.in_callback(thread) => {
                return self.nested(request(Depth::Nested), host);
            }
            _ => return Err(EngineError::Busy),
        }
        let block = &self.block;
        let dispatch = block.term.begin(kind).map_err(|error| match error {
            TermError::Disposing => EngineError::Disposed,
            TermError::AlreadyArmed => EngineError::Busy,
        })?;
        // mini_racer times only scripts, never setup or maintenance.
        if let (DispatchKind::Script, Some(timeout)) = (kind, block.config.timeout)
            && let Err(error) = watchdog::schedule(timeout, block, dispatch)
        {
            block.term.end(dispatch);
            return Err(error);
        }
        self.session.owner.store(thread, Ordering::Release);
        let owning = Owning(&self.session.owner);
        let result = match block.channel.request(request(Depth::Outermost(dispatch))) {
            Ok(()) => self.await_response(false, host),
            Err(error) => Err(channel(error)),
        };
        drop(owning);
        // A no-op unless the runner died before disarming it.
        block.term.end(dispatch);
        watchdog::clear(block, dispatch);
        result
    }

    fn nested(&self, request: Request, host: &mut impl Host) -> Result<Vec<u8>, EngineError> {
        self.block.channel.request(request).map_err(channel)?;
        let _waiting = Counted::enter(&self.session.waits);
        self.await_response(true, host)
    }

    // Once the dispatch is abandoned, callbacks still in flight are answered
    // with `Abort` without running, until the runner responds. Abandoning a
    // nested dispatch stops only its script, as mini_racer cancels only the
    // nested call: the dispatch it runs in carries on once it has unwound.
    fn await_response(&self, nested: bool, host: &mut impl Host) -> Result<Vec<u8>, EngineError> {
        let block = &self.block;
        let wait = WaitHandle {
            channel: &block.channel,
        };
        let mut cancelled = false;
        let abandon = |cancelled: &mut bool| {
            *cancelled = true;
            if nested {
                block.term.interrupt();
            } else if let Some(dispatch) = block.term.armed() {
                block.term.request(Interrupt::Cancel(dispatch));
            }
        };
        loop {
            if cancelled {
                host.wait_blocking(&wait);
            } else if host.wait(&wait) == WaitOutcome::Cancel {
                abandon(&mut cancelled);
                continue;
            }
            match block.channel.recv_ruby() {
                Ok(Some(ToRuby::Response(response))) => {
                    if nested && cancelled {
                        block.term.release();
                    }
                    return response;
                }
                Ok(Some(ToRuby::Callback(request))) => {
                    let reply = if cancelled {
                        CallbackReply::Abort
                    } else {
                        let _running = Counted::enter(&self.session.callbacks);
                        host.callback(&request)
                    };
                    if matches!(reply, CallbackReply::Abort) && !cancelled {
                        abandon(&mut cancelled);
                    }
                    block.channel.reply(reply).map_err(channel)?;
                }
                Ok(None) => {}
                Err(error) => return Err(channel(error)),
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

fn channel(error: ChannelError) -> EngineError {
    match error {
        ChannelError::Closed => EngineError::Disposed,
        error => protocol(error),
    }
}
