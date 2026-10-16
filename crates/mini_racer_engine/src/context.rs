use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use mini_racer_sync::{
    Channel, ChannelError, DispatchKind, Interrupt, Phase, Side, TermError, ToRuby, lock,
};

use crate::control::{CallbackReply, ContextConfig, ControlBlock, Depth, JsText, Request, Wire};
use crate::process::fork::{self, Generation};
use crate::process::platform::Mode;
use crate::process::{self, platform, watchdog};
use crate::runner::Start;
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
    /// dispatch, and is all a host that attaches nothing needs.
    fn callback(&mut self, _request: &[u8]) -> CallbackReply {
        CallbackReply::Abort
    }
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
    lineage: Lineage,
    session: Session,
}

// The control block this handle uses in each process of its fork lineage. A
// child that takes an idle context over gives it a block of its own and never
// touches the inherited one, whose locks threads that are gone may hold.
struct Lineage {
    pid: u32,
    held: Held,
    next: OnceLock<Box<Self>>,
}

enum Held {
    Block(Arc<ControlBlock>),
    // Disposed before the fork that led here.
    Disposed,
    // In use at the fork, or after a fork that did not wait for the runners.
    Orphaned,
}

impl Lineage {
    // The link for `pid`, without making one.
    fn find(&self, pid: u32) -> Result<&Self, &Self> {
        let mut lineage = self;
        while lineage.pid != pid {
            match lineage.next.get() {
                Some(next) => lineage = next,
                None => return Err(lineage),
            }
        }
        Ok(lineage)
    }
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
    pub fn spawn(mut config: ContextConfig, host: &mut impl Host) -> Result<Self, EngineError> {
        let snapshot = config.snapshot.take();
        process::ensure_initialized();
        let generation = fork::current();
        let block = Arc::new(ControlBlock::new(config));
        start(&block, generation, Start::Fresh(snapshot))?;
        let handle = Self {
            lineage: Lineage {
                pid: std::process::id(),
                held: Held::Block(block),
                next: OnceLock::new(),
            },
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

    /// Builds a startup snapshot from `code`, or warms `base` up with it, on
    /// a context made for the purpose and disposed after, as `mini_racer`
    /// does.
    pub fn create_snapshot(
        code: JsText,
        base: Option<Vec<u8>>,
        host: &mut impl Host,
    ) -> Result<Vec<u8>, EngineError> {
        let context = Self::spawn(ContextConfig::default(), host)?;
        let built = context.dispatch(
            DispatchKind::Script,
            |depth| Request::Snapshot { code, base, depth },
            host,
        );
        context.begin_dispose();
        let disposed = context.dispose(host);
        let blob = built?;
        disposed.map(|()| blob)
    }

    /// Whether this thread is running one of this context's callbacks, where a
    /// dispatch runs nested in the one that made the callback.
    #[must_use]
    pub fn in_callback(&self) -> bool {
        self.session.in_callback(process::thread_token())
    }

    /// Dispatches open on this context, the outermost one included.
    #[must_use]
    pub fn depth(&self) -> u32 {
        if self.session.owner.load(Ordering::Acquire) == 0 {
            0
        } else {
            self.session.waits.load(Ordering::Relaxed).saturating_add(1)
        }
    }

    /// Terminates the running script, or the next one if none is running.
    pub fn stop(&self) -> Result<(), EngineError> {
        match self.held() {
            Held::Block(_) if self.is_disposed() => Err(EngineError::Disposed),
            Held::Block(block) => {
                block.term.request(Interrupt::Stop);
                Ok(())
            }
            Held::Disposed => Err(EngineError::Disposed),
            Held::Orphaned => Ok(()),
        }
    }

    /// Refuses new dispatches and terminates one in flight on another thread.
    /// The caller then waits for that dispatch to finish before `dispose`.
    pub fn begin_dispose(&self) {
        if let Held::Block(block) = self.held() {
            block.term.dispose();
        }
    }

    /// Shuts the runner down and returns once its isolate is gone. Expects
    /// `begin_dispose` and no dispatch in flight; a second call is a no-op.
    pub fn dispose(&self, host: &mut impl Host) -> Result<(), EngineError> {
        let block = match self.held() {
            Held::Block(block) => block,
            Held::Disposed => return Ok(()),
            // mini_racer refuses a single-threaded context a child inherited in use.
            Held::Orphaned => {
                return match platform::mode() {
                    Mode::SingleThreaded => Err(EngineError::DisposeBusy),
                    Mode::MultiThreaded => Ok(()),
                };
            }
        };
        if self.session.callbacks.load(Ordering::Relaxed) > 0 {
            return Err(EngineError::DisposeBusy);
        }
        block.term.dispose();
        match block.channel.request(Request::Dispose) {
            Ok(()) => {}
            Err(ChannelError::Closed) => return Ok(()),
            Err(error) => return Err(protocol(error)),
        }
        let wait = WaitHandle {
            channel: &block.channel,
        };
        loop {
            host.wait_blocking(&wait);
            match block.channel.recv_ruby() {
                Ok(Some(ToRuby::Response(response))) => return response.map(drop),
                Err(ChannelError::Closed) => return Ok(()),
                Ok(Some(ToRuby::Callback(_))) => {
                    let _ = block.channel.reply(CallbackReply::Abort);
                }
                Ok(None) => {}
                Err(error) => return Err(protocol(error)),
            }
        }
    }

    /// Answers without taking an inherited context over.
    #[must_use]
    pub fn is_disposed(&self) -> bool {
        match self.lineage.find(std::process::id()) {
            Ok(lineage) => match &lineage.held {
                Held::Block(block) => block.channel.is_closed() || block.term.is_disposing(),
                Held::Disposed => true,
                Held::Orphaned => false,
            },
            Err(inherited) => match &inherited.held {
                Held::Block(block) => block.channel.is_closed(),
                Held::Disposed => true,
                Held::Orphaned => false,
            },
        }
    }

    // What this process holds of the context, taking it over first in a
    // forked child.
    fn held(&self) -> &Held {
        let pid = std::process::id();
        let mut lineage = &self.lineage;
        while lineage.pid != pid {
            let inherited = &lineage.held;
            lineage = lineage
                .next
                .get_or_init(|| Box::new(self.inherit(inherited, pid)));
        }
        &lineage.held
    }

    fn inherit(&self, inherited: &Held, pid: u32) -> Lineage {
        let held = match inherited {
            // The closed flag is an atomic, safe to read in what was inherited.
            Held::Block(block) if block.channel.is_closed() => Held::Disposed,
            Held::Block(block) => adopt(block).map_or(Held::Orphaned, Held::Block),
            Held::Disposed => Held::Disposed,
            Held::Orphaned => Held::Orphaned,
        };
        if matches!(held, Held::Block(_)) {
            // A thread of the parent may have claimed the context just before
            // the fork, without sending anything.
            self.session.owner.store(0, Ordering::Release);
            self.session.callbacks.store(0, Ordering::Relaxed);
            self.session.waits.store(0, Ordering::Relaxed);
        }
        Lineage {
            pid,
            held,
            next: OnceLock::new(),
        }
    }

    fn dispatch(
        &self,
        kind: DispatchKind,
        request: impl FnOnce(Depth) -> Request,
        host: &mut impl Host,
    ) -> Result<Vec<u8>, EngineError> {
        let block = match self.held() {
            Held::Block(block) => block,
            Held::Disposed => return Err(EngineError::Disposed),
            Held::Orphaned => return Err(EngineError::Orphaned),
        };
        let thread = process::thread_token();
        match self.session.owner.load(Ordering::Acquire) {
            0 => {}
            _ if self.session.in_callback(thread) => {
                return self.nested(block, request(Depth::Nested), host);
            }
            _ => return Err(EngineError::Busy),
        }
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
            Ok(()) => self.await_response(block, false, host),
            Err(error) => Err(channel(error)),
        };
        drop(owning);
        // A no-op unless the runner died before disarming it.
        block.term.end(dispatch);
        watchdog::clear(block, dispatch);
        result
    }

    fn nested(
        &self,
        block: &ControlBlock,
        request: Request,
        host: &mut impl Host,
    ) -> Result<Vec<u8>, EngineError> {
        if block.term.is_disposing() {
            return Err(EngineError::Disposed);
        }
        block.channel.request(request).map_err(channel)?;
        let _waiting = Counted::enter(&self.session.waits);
        self.await_response(block, true, host)
    }

    // Once the dispatch is abandoned, callbacks still in flight are answered
    // with `Abort` without running, until the runner responds. Abandoning a
    // nested dispatch stops only its script, as mini_racer cancels only the
    // nested call: the dispatch it runs in carries on once it has unwound. A
    // dispose that lands while a callback runs ends the dispatch as disposed,
    // as mini_racer's callback finds its context gone.
    fn await_response(
        &self,
        block: &ControlBlock,
        nested: bool,
        host: &mut impl Host,
    ) -> Result<Vec<u8>, EngineError> {
        let wait = WaitHandle {
            channel: &block.channel,
        };
        let mut cancelled = false;
        let mut disposed = false;
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
                    return if disposed {
                        Err(EngineError::Disposed)
                    } else {
                        response
                    };
                }
                Ok(Some(ToRuby::Callback(request))) => {
                    let reply = if cancelled {
                        CallbackReply::Abort
                    } else {
                        let _running = Counted::enter(&self.session.callbacks);
                        let reply = host.callback(&request);
                        disposed = block.term.is_disposing();
                        if disposed {
                            CallbackReply::Abort
                        } else {
                            reply
                        }
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
    // An inherited context this process never took over is left as it is: its
    // locks may have been held by parent threads that do not exist here.
    fn drop(&mut self) {
        if let Ok(Lineage {
            held: Held::Block(block),
            ..
        }) = self.lineage.find(std::process::id())
        {
            block.term.dispose();
            block.channel.close();
        }
    }
}

fn start(
    block: &Arc<ControlBlock>,
    generation: &'static Generation,
    start: Start,
) -> Result<(), EngineError> {
    // Registered before the runner starts, so a fork either waits for it or
    // finds it waiting for the fork.
    generation.register(block);
    let runner_block = Arc::clone(block);
    let stack = platform::stack_size_kb().map_or(MIN_RUNNER_STACK_BYTES, |kb| {
        (kb.saturating_mul(1024).saturating_add(STACK_HEADROOM_BYTES)).max(MIN_RUNNER_STACK_BYTES)
    });
    std::thread::Builder::new()
        .name("mini_racer_rs".to_owned())
        .stack_size(stack)
        .spawn(move || runner::run(&runner_block, generation, start))
        .map_err(|e| EngineError::Internal(format!("spawn runner: {e}")))?;
    Ok(())
}

// A forked child takes over a context whose runner was idle, with nothing in
// flight, at a fork that waited for every runner; any other stays the parent's.
fn adopt(inherited: &ControlBlock) -> Option<Arc<ControlBlock>> {
    let generation = fork::current();
    if !generation.quiesced()
        || inherited.phase.get() != Phase::Deposited
        || !inherited.channel.idle()
    {
        return None;
    }
    let isolate = lock(&inherited.custody).take()?;
    let block = Arc::new(ControlBlock::new(inherited.config.clone()));
    *lock(&block.custody) = Some(isolate);
    start(&block, generation, Start::Adopted).ok()?;
    Some(block)
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
