use std::io;
use std::os::fd::OwnedFd;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use mini_racer_sync::{
    Channel, ChannelError, DispatchKind, Inherited, Interrupt, Phase, Side, TermError, ToRuby, lock,
};

use crate::control::{CallbackReply, ContextConfig, ControlBlock, Depth, JsText, Request, Wire};
use crate::notify::NotifyFd;
use crate::process::fork::{self, Generation};
use crate::process::platform::Mode;
use crate::process::{self, platform, watchdog};
use crate::runner::Start;
use crate::spin::spin_until;
use crate::{EngineError, InvalidSnapshot, SnapshotBlob, runner};

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
    notify: &'a NotifyFd,
    epoch: u64,
}

impl<'a> WaitHandle<'a> {
    fn new(block: &'a ControlBlock) -> Self {
        Self {
            channel: &block.channel,
            notify: &block.notify,
            epoch: fork::epoch(),
        }
    }

    // A fork from a callback leaves the child waiting on a runner it does not
    // have; it stops waiting and the dispatch ends orphaned.
    fn forked(&self) -> bool {
        fork::epoch() != self.epoch
    }

    #[must_use]
    pub fn ready(&self) -> bool {
        self.channel.ready(Side::Ruby) || self.forked()
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

    /// `park` for a host that sleeps on a file descriptor instead, such as a
    /// fiber scheduler: `wait` should return once a `notifier` descriptor is
    /// readable, which it becomes when the runner answers, and may return
    /// earlier.
    pub fn park_notified(&self, wait: impl FnOnce()) -> io::Result<()> {
        // The descriptor is shared with the parent: draining it in a child
        // would take the parent's ring.
        if self.forked() {
            return Ok(());
        }
        // Drained before the channel is marked, so a ring for this handover
        // always comes after the drain.
        self.notify.drain(self.channel)?;
        self.channel.park_notified(Side::Ruby, wait);
        Ok(())
    }

    /// A descriptor the host owns that `park_notified` waits are rung on.
    pub fn notifier(&self) -> io::Result<OwnedFd> {
        if self.forked() {
            return Err(io::Error::other(
                "the context's runner stayed with the parent process",
            ));
        }
        self.notify.duplicate(self.channel)
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
    epoch: u64,
    held: Held,
    next: OnceLock<Box<Self>>,
}

enum Held {
    Block(Arc<ControlBlock>),
    // Disposed, or being disposed, before the fork that led here.
    Disposed,
    Orphaned(Orphan),
}

// An inherited context left to the parent.
struct Orphan {
    in_use: bool,
    disposed: AtomicBool,
}

const fn orphaned(in_use: bool, disposed: bool) -> Held {
    Held::Orphaned(Orphan {
        in_use,
        disposed: AtomicBool::new(disposed),
    })
}

impl Orphan {
    fn disposed(&self) -> bool {
        self.disposed.load(Ordering::Acquire)
    }
}

impl Lineage {
    // The link for `epoch`, or else the last one, without making one.
    fn find(&self, epoch: u64) -> Result<&Self, &Self> {
        let mut lineage = self;
        while lineage.epoch != epoch {
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
    pub fn spawn(config: ContextConfig, host: &mut impl Host) -> Result<Self, EngineError> {
        process::ensure_initialized();
        // Read here, on the caller's thread, not first on a runner's.
        let _ = process::code_cache::enabled();
        let _ = process::fast_calls();
        let generation = fork::current();
        let block = Arc::new(ControlBlock::new(config));
        start(&block, generation, Start::Fresh)?;
        let handle = Self {
            lineage: Lineage {
                epoch: fork::epoch(),
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
        self.evaluate((filename.into(), source.into()), false, host)
    }

    /// `eval`, then awaits a promise result: runs microtasks and due tasks
    /// until it settles, as `mini_racer`'s `eval_await` does.
    pub fn eval_await(
        &self,
        filename: impl Into<JsText>,
        source: impl Into<JsText>,
        host: &mut impl Host,
    ) -> Result<Vec<u8>, EngineError> {
        self.evaluate((filename.into(), source.into()), true, host)
    }

    fn evaluate(
        &self,
        (filename, source): (JsText, JsText),
        awaited: bool,
        host: &mut impl Host,
    ) -> Result<Vec<u8>, EngineError> {
        self.dispatch(
            DispatchKind::Script,
            |depth| Request::Eval {
                filename,
                source,
                depth,
                awaited,
            },
            host,
        )
    }

    /// `request` is the wire form `mini_racer` sends: one array holding the
    /// function's dotted path, then its arguments.
    pub fn call(&self, request: Vec<u8>, host: &mut impl Host) -> Result<Vec<u8>, EngineError> {
        self.invoke(request, false, host)
    }

    /// `call`, then awaits a promise result, as `eval_await` does.
    pub fn call_await(
        &self,
        request: Vec<u8>,
        host: &mut impl Host,
    ) -> Result<Vec<u8>, EngineError> {
        self.invoke(request, true, host)
    }

    fn invoke(
        &self,
        request: Vec<u8>,
        awaited: bool,
        host: &mut impl Host,
    ) -> Result<Vec<u8>, EngineError> {
        self.dispatch(
            DispatchKind::Script,
            |depth| Request::Call {
                request,
                depth,
                awaited,
            },
            host,
        )
    }

    /// Runs the microtasks queued so far.
    pub fn perform_microtask_checkpoint(&self, host: &mut impl Host) -> Result<(), EngineError> {
        self.dispatch(
            DispatchKind::Maintenance,
            |depth| Request::MicrotaskCheckpoint { depth },
            host,
        )
        .map(drop)
    }

    /// V8's heap statistics under `mini_racer`'s names: a wire object whose
    /// values are numbers.
    pub fn heap_stats(&self, host: &mut impl Host) -> Result<Vec<u8>, EngineError> {
        self.dispatch(
            DispatchKind::Maintenance,
            |depth| Request::HeapStats { depth },
            host,
        )
    }

    /// A heap snapshot, as the JSON V8 writes.
    pub fn heap_snapshot(&self, host: &mut impl Host) -> Result<Vec<u8>, EngineError> {
        self.dispatch(
            DispatchKind::Maintenance,
            |depth| Request::HeapSnapshot { depth },
            host,
        )
    }

    /// Runs one due task of V8's, then the microtasks it queued; returns
    /// whether a task ran.
    pub fn pump_message_loop(&self, host: &mut impl Host) -> Result<bool, EngineError> {
        self.dispatch(
            DispatchKind::Pump,
            |depth| Request::PumpMessageLoop { depth },
            host,
        )
        .map(|ran| ran == [1])
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

    /// Asks V8 to collect as much garbage as it can; returns once it has.
    pub fn low_memory_notification(&self, host: &mut impl Host) -> Result<(), EngineError> {
        self.dispatch(
            DispatchKind::Maintenance,
            |depth| Request::LowMemoryNotification { depth },
            host,
        )
        .map(drop)
    }

    /// Builds a startup snapshot from `code`, or warms `base` up with it, on
    /// a context made for the purpose and disposed after, as `mini_racer`
    /// does.
    pub fn create_snapshot(
        code: JsText,
        base: Option<SnapshotBlob>,
        host: &mut impl Host,
    ) -> Result<Vec<u8>, EngineError> {
        // A warmup from no snapshot would build the read-only heap again, which
        // V8 aborts on once the process has one.
        if base.as_ref().is_some_and(SnapshotBlob::is_empty) {
            return Err(InvalidSnapshot::Empty.into());
        }
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
            Held::Orphaned(orphan) if orphan.disposed() => Err(EngineError::Disposed),
            Held::Orphaned(_) => Ok(()),
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
            Held::Orphaned(orphan) if orphan.in_use && platform::mode() == Mode::SingleThreaded => {
                return Err(EngineError::DisposeBusy);
            }
            Held::Orphaned(orphan) => {
                orphan.disposed.store(true, Ordering::Release);
                return Ok(());
            }
        };
        if self.session.callbacks.load(Ordering::Relaxed) > 0 {
            return Err(EngineError::DisposeBusy);
        }
        block.term.dispose();
        let epoch = fork::epoch();
        let result = Self::shut_down(block, host);
        // A child that forked during the wait leaves the parent's descriptor
        // alone, as it does every lock it inherited.
        if fork::epoch() == epoch {
            block.notify.release(&block.channel);
        }
        result
    }

    fn shut_down(block: &ControlBlock, host: &mut impl Host) -> Result<(), EngineError> {
        match block.channel.request(Request::Dispose) {
            Ok(()) => {}
            Err(ChannelError::Closed) => return Ok(()),
            Err(error) => return Err(protocol(error)),
        }
        let wait = WaitHandle::new(block);
        loop {
            host.wait_blocking(&wait);
            if wait.forked() {
                // The child finds the context disposing, so disposed.
                return Ok(());
            }
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
        match self.lineage.find(fork::epoch()) {
            Ok(lineage) => match &lineage.held {
                Held::Block(block) => block.channel.is_closed() || block.term.is_disposing(),
                Held::Disposed => true,
                Held::Orphaned(orphan) => orphan.disposed(),
            },
            Err(inherited) => match &inherited.held {
                Held::Block(block) => {
                    block.channel.is_closed()
                        || block
                            .term
                            .inherited()
                            .is_some_and(|inherited| inherited.disposing)
                }
                Held::Disposed => true,
                Held::Orphaned(orphan) => orphan.disposed(),
            },
        }
    }

    // What this process holds of the context, taking it over first in a
    // forked child.
    // A process the context passed through unused (a daemon's intermediate
    // child) gets no link: its fork left the block exactly as inherited, and
    // the generation's quiesced flag already covers every fork on the way.
    fn held(&self) -> &Held {
        let epoch = fork::epoch();
        match self.lineage.find(epoch) {
            Ok(lineage) => &lineage.held,
            Err(last) => {
                &last
                    .next
                    .get_or_init(|| Box::new(self.inherit(&last.held, epoch)))
                    .held
            }
        }
    }

    // Only atomics and try-locks touch what was inherited.
    fn inherit(&self, inherited: &Held, epoch: u64) -> Lineage {
        let held = match inherited {
            Held::Block(block) if block.channel.is_closed() => Held::Disposed,
            Held::Block(block) => match block.term.inherited() {
                Some(inherited) if inherited.disposing => Held::Disposed,
                // A parent thread may have claimed the context and not sent yet.
                Some(inherited) if self.session.owner.load(Ordering::Acquire) == 0 => {
                    adopt(block, inherited)
                }
                _ => orphaned(true, false),
            },
            Held::Disposed => Held::Disposed,
            Held::Orphaned(orphan) => orphaned(orphan.in_use, orphan.disposed()),
        };
        Lineage {
            epoch,
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
            Held::Orphaned(orphan) if orphan.disposed() => return Err(EngineError::Disposed),
            Held::Orphaned(_) => return Err(EngineError::Orphaned),
        };
        let thread = process::thread_token();
        match self.session.owner.load(Ordering::Acquire) {
            0 => {}
            _ if self.session.in_callback(thread) => {
                return self.nested(block, kind, request(Depth::Nested), host);
            }
            _ => return Err(EngineError::Busy),
        }
        let dispatch = block.term.begin(kind).map_err(|error| match error {
            TermError::Disposing => EngineError::Disposed,
            TermError::AlreadyArmed => EngineError::Busy,
        })?;
        self.session.owner.store(thread, Ordering::Release);
        let owning = Owning(&self.session.owner);
        let wait = WaitHandle::new(block);
        let result = match block.channel.request(request(Depth::Outermost(dispatch))) {
            Ok(()) => self.await_response(block, &wait, false, host),
            Err(error) => Err(channel(error)),
        };
        drop(owning);
        if wait.forked() {
            // The runner and the watchdog that armed the dispatch are the
            // parent's, and may have held its locks at the fork.
            return result;
        }
        // A no-op unless the runner died before disarming it.
        block.term.end(dispatch);
        watchdog::clear(block, dispatch);
        result
    }

    // A script nested in an untimed dispatch gets a timeout of its own.
    fn nested(
        &self,
        block: &Arc<ControlBlock>,
        kind: DispatchKind,
        request: Request,
        host: &mut impl Host,
    ) -> Result<Vec<u8>, EngineError> {
        if block.term.is_disposing() {
            return Err(EngineError::Disposed);
        }
        let timer = match (kind, block.config.timeout) {
            (DispatchKind::Script, Some(timeout)) => {
                block.term.time_nested().map(|id| (timeout, id))
            }
            _ => None,
        };
        if let Some((timeout, id)) = timer
            && let Err(error) = watchdog::schedule(timeout, block, id)
        {
            block.term.untime_nested(id);
            return Err(error);
        }
        let wait = WaitHandle::new(block);
        let result = block
            .channel
            .request(request)
            .map_err(channel)
            .and_then(|()| {
                let _waiting = Counted::enter(&self.session.waits);
                self.await_response(block, &wait, true, host)
            });
        if let Some((_, id)) = timer
            && !wait.forked()
        {
            watchdog::clear(block, id);
            block.term.untime_nested(id);
        }
        result
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
        wait: &WaitHandle<'_>,
        nested: bool,
        host: &mut impl Host,
    ) -> Result<Vec<u8>, EngineError> {
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
            let outcome = if cancelled {
                host.wait_blocking(wait);
                WaitOutcome::Ready
            } else {
                host.wait(wait)
            };
            if wait.forked() {
                return Err(EngineError::Orphaned);
            }
            if outcome == WaitOutcome::Cancel {
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
                        if wait.forked() {
                            return Err(EngineError::Orphaned);
                        }
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
        }) = self.lineage.find(fork::epoch())
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
// A stop or memory cause left for the next script carries over, as in
// mini_racer.
fn adopt(inherited: &ControlBlock, carried: Inherited) -> Held {
    if inherited.phase.get() != Phase::Deposited || !inherited.channel.idle() {
        return orphaned(true, false);
    }
    let generation = fork::current();
    if !generation.quiesced() {
        return orphaned(false, false);
    }
    let Some(tasks) = inherited.tasks.inherited() else {
        return orphaned(true, false);
    };
    let Some(isolate) = inherited
        .custody
        .try_lock()
        .ok()
        .and_then(|mut custody| custody.take())
    else {
        return orphaned(true, false);
    };
    let block = Arc::new(ControlBlock::with_tasks(inherited.config.clone(), tasks));
    *lock(&block.custody) = Some(isolate);
    block.term.adopt(carried);
    // The isolate keeps whatever termination V8 still held for it.
    block.termination_requested.store(
        inherited.termination_requested.load(Ordering::Acquire),
        Ordering::Release,
    );
    match start(&block, generation, Start::Adopted) {
        Ok(()) => Held::Block(block),
        Err(_) => orphaned(true, false),
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
