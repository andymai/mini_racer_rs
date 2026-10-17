use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mini_racer_sync::{
    DispatchId, DispatchKind, Halt, Interrupt, Phase, Side, TerminationCause, ToV8, lock,
};
use mini_racer_wire::Writer;
use v8::ValueDeserializerHelper;

use crate::control::{CallbackReply, ControlBlock, Depth, IsolateTerminator, JsText, Request};
use crate::process::fork::Generation;
use crate::process::platform::{self, Mode};
use crate::process::watchdog;
use crate::serialize::{self, Filter, Tc};
use crate::spin::spin_until;
use crate::{EngineError, JsError, JsErrorKind};
use crate::{snapshot, sys};

// Spinning after each response catches a busy caller's next request without a
// futex wake on either side; an idle context then parks.
const IDLE_SPIN: Duration = Duration::from_micros(100);

// Closes the channel however the runner ends, so a waiting Ruby thread never
// parks on a runner that is gone.
struct Closing<'a>(&'a ControlBlock);

impl Drop for Closing<'_> {
    fn drop(&mut self) {
        self.0.term.uninstall();
        self.0.channel.close();
    }
}

// Kept in an isolate slot: the callback trampoline gets only the isolate. A
// shared isolate's slots must be Send, as any thread may lock it.
struct Runtime {
    block: Arc<ControlBlock>,
    generation: &'static Generation,
    // Whether the runner holds its isolate only while serving a request.
    released: bool,
    context: v8::Global<v8::Context>,
    filter: Filter,
    // The exception last thrown for a raising callback; mini_racer keeps one
    // per isolate and compares what escapes a dispatch against it.
    raised: Mutex<Option<v8::Global<v8::Value>>>,
    // The address V8 posts this isolate's tasks under.
    key: usize,
    // Tasks running, one inside another when a task's script pumps.
    running: AtomicU32,
}

impl Runtime {
    fn new(
        block: &Arc<ControlBlock>,
        generation: &'static Generation,
        (context, filter): (v8::Global<v8::Context>, Filter),
        key: usize,
    ) -> Self {
        Self {
            block: Arc::clone(block),
            generation,
            released: platform::mode() == Mode::SingleThreaded,
            context,
            filter,
            raised: Mutex::default(),
            key,
            running: AtomicU32::new(0),
        }
    }
}

struct Running<'a>(&'a AtomicU32);

impl Drop for Running<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

/// How a runner gets its isolate: made fresh, optionally from a snapshot, or
/// taken over in a forked child from an idle runner of the parent.
pub enum Start {
    Fresh(Option<Vec<u8>>),
    /// The isolate waits in the new control block's custody.
    Adopted,
}

pub fn run(block: &Arc<ControlBlock>, generation: &'static Generation, start: Start) {
    let closing = Closing(block);
    let disposed = catch_unwind(AssertUnwindSafe(|| match start {
        Start::Fresh(snapshot) => serve(block, generation, snapshot),
        Start::Adopted => {
            let isolate = lock(&block.custody).take();
            isolate.map_or_else(
                || refuse(block, "no isolate to take over"),
                |isolate| adopt(block, generation, isolate),
            )
        }
    }))
    .unwrap_or_else(|_| {
        if block.channel.ready(Side::V8) {
            let _ = block
                .channel
                .respond(Err(EngineError::Internal("the runner panicked".to_owned())));
        }
        false
    });
    // Answering only after the isolate is gone makes an explicit dispose return
    // once V8 has released everything.
    if disposed {
        let _ = block.channel.respond(Ok(Vec::new()));
    }
    drop(closing);
    block.phase.set(Phase::Exited);
}

// mini_racer's max_memory, checked after every garbage collection: a heap
// past it terminates the running script, or the next one.
pub fn check_heap(isolate: &mut v8::Isolate) {
    let Some(runtime) = isolate.get_slot::<Arc<Runtime>>().cloned() else {
        return;
    };
    let Some(max) = runtime.block.config.max_memory else {
        return;
    };
    let used = isolate.get_heap_statistics().used_heap_size();
    if u64::try_from(used).unwrap_or(u64::MAX) > max {
        runtime.block.term.request(Interrupt::MaxMemory);
    }
}

pub fn park_for_fork(isolate: &v8::Isolate) {
    if let Some(runtime) = isolate.get_slot::<Arc<Runtime>>() {
        runtime.generation.pause(&runtime.block);
    }
}

fn refuse(block: &ControlBlock, message: &str) -> bool {
    // Holding no isolate, it never keeps a fork waiting.
    block.phase.set(Phase::Deposited);
    if receive(block).is_some() {
        let _ = block
            .channel
            .respond(Err(EngineError::Internal(message.to_owned())));
    }
    false
}

// Returns whether it stopped for an explicit dispose.
fn serve(
    block: &Arc<ControlBlock>,
    generation: &'static Generation,
    snapshot: Option<Vec<u8>>,
) -> bool {
    // No isolate is made while a fork waits for every runner to be idle.
    if platform::mode() == Mode::SingleThreaded {
        generation.resume(block);
    }
    let from_snapshot = snapshot.is_some();
    let params = v8::CreateParams::default();
    let params = match snapshot {
        Some(blob) => params.snapshot_blob(v8::StartupData::from(blob)),
        None => params,
    };
    let mut isolate = v8::Isolate::new(params);
    if block.config.max_memory.is_some() {
        sys::watch_heap(&mut isolate);
    }
    let key = sys::isolate_key(&isolate);
    generation.tasks().register(key, &block.tasks);
    let contexts = {
        v8::scope!(let scope, &mut isolate);
        serialize::contexts(scope, from_snapshot)
    };
    let Some(contexts) = contexts else {
        retire(block, generation, key, isolate);
        return refuse(block, "context setup failed");
    };
    let runtime = Arc::new(Runtime::new(block, generation, contexts, key));
    isolate.set_slot(Arc::clone(&runtime));
    // Installed only once the context exists, so nothing terminates its setup.
    block.term.install(IsolateTerminator::new(
        isolate.thread_safe_handle(),
        &block.tasks,
    ));
    let shared = match sys::share(isolate) {
        Ok(shared) => shared,
        Err(refused) => {
            retire(block, generation, key, refused);
            return refuse(block, "isolate could not be shared");
        }
    };
    let _ = block.isolate.set(shared.thread_safe_handle());
    serve_isolate(block, shared, runtime)
}

// The parent's runtime still names the parent's control block, so the child
// builds its own around the same contexts.
fn adopt(
    block: &Arc<ControlBlock>,
    generation: &'static Generation,
    shared: v8::SharedIsolate,
) -> bool {
    generation.resume(block);
    let inherited = shared.lock().get_slot::<Arc<Runtime>>().cloned();
    let Some(inherited) = inherited else {
        // Disposed here, before the runner counts as idle to a fork.
        drop(block.tasks.close());
        drop(shared);
        return refuse(block, "inherited isolate has no context");
    };
    let runtime = {
        let mut locker = shared.lock();
        let key = sys::isolate_key(&locker);
        generation.tasks().register(key, &block.tasks);
        let runtime = Arc::new(Runtime::new(
            block,
            generation,
            (inherited.context.clone(), inherited.filter.clone()),
            key,
        ));
        locker.set_slot(Arc::clone(&runtime));
        runtime
    };
    block.term.install(IsolateTerminator::new(
        shared.thread_safe_handle(),
        &block.tasks,
    ));
    let _ = block.isolate.set(shared.thread_safe_handle());
    serve_isolate(block, shared, runtime)
}

fn serve_isolate(block: &ControlBlock, shared: v8::SharedIsolate, runtime: Arc<Runtime>) -> bool {
    let (generation, key) = (runtime.generation, runtime.key);
    let (disposed, shared) = if runtime.released {
        serve_released(block, shared, &runtime)
    } else {
        (serve_held(block, &shared, &runtime), Some(shared))
    };
    block.term.uninstall();
    // Its handles are released under the lock, before the isolate goes.
    match shared {
        Some(shared) => {
            let mut isolate = shared.lock();
            drop(isolate.remove_slot::<Arc<Runtime>>());
            drop(runtime);
            drop(isolate);
            retire(block, generation, key, shared);
        }
        None => retire(block, generation, key, ()),
    }
    disposed
}

// Queued tasks are dropped while their isolate exists. Its entry stays until
// the isolate is gone, so a task V8 posts during the teardown meets the closed
// queue and is dropped too, never kept for a later isolate at that address.
fn retire<I>(block: &ControlBlock, generation: &Generation, key: usize, isolate: I) {
    drop(block.tasks.close());
    drop(isolate);
    generation.tasks().unregister(key, &block.tasks);
}

enum Served {
    Next,
    Stop { disposed: bool },
}

// With `gc_after_idle`, mini_racer's runner asks V8 for a full collection
// once it has waited that long after a request, and not again until the next.
fn serve_held(block: &ControlBlock, shared: &v8::SharedIsolate, runtime: &Arc<Runtime>) -> bool {
    let mut isolate = shared.lock();
    let mut collected = true;
    loop {
        let message = match block.config.gc_after_idle {
            Some(idle) if !collected => match receive_within(block, idle) {
                Received::Message(message) => message,
                // A collection for a context about to be torn down is wasted.
                Received::Idle => {
                    if !block.term.is_disposing() {
                        isolate.low_memory_notification();
                        sys::release_free_memory();
                    }
                    collected = true;
                    continue;
                }
            },
            _ => receive(block),
        };
        // mini_racer's context has sent nothing before its first request.
        if !matches!(message, Some(ToV8::Request(Request::Init { .. }))) {
            collected = false;
        }
        if let Served::Stop { disposed } = serve_one(&mut isolate, block, runtime, message) {
            return disposed;
        }
    }
}

// Between requests a single-threaded runner leaves its isolate unlocked in the
// control block, as mini_racer's holds its isolate only for a request.
fn serve_released(
    block: &ControlBlock,
    shared: v8::SharedIsolate,
    runtime: &Arc<Runtime>,
) -> (bool, Option<v8::SharedIsolate>) {
    let mut held = Some(shared);
    loop {
        *lock(&block.custody) = held.take();
        block.phase.set(Phase::Deposited);
        let message = receive(block);
        runtime.generation.resume(block);
        let Some(shared) = lock(&block.custody).take() else {
            return (false, None);
        };
        let mut locked = shared.lock();
        block.term.relocked();
        let served = serve_one(&mut locked, block, runtime, message);
        drop(locked);
        held = Some(shared);
        if let Served::Stop { disposed } = served {
            return (disposed, held);
        }
    }
}

fn serve_one(
    isolate: &mut v8::Isolate,
    block: &ControlBlock,
    runtime: &Arc<Runtime>,
    message: Option<ToV8<crate::control::Wire>>,
) -> Served {
    let request = match message {
        Some(ToV8::Request(Request::Dispose)) => return Served::Stop { disposed: true },
        // The snapshot's own isolate is entered with none of this
        // runner's scopes open.
        // A build the caller abandoned before it began is not started; once
        // begun it runs to the end, as nothing can terminate its isolate.
        Some(ToV8::Request(Request::Snapshot { code, base, depth })) => {
            let response = match depth {
                Depth::Outermost(dispatch) => {
                    let built = if block.term.check(DispatchKind::Script).is_some() {
                        Err(EngineError::Internal("halted without a cause".to_owned()))
                    } else {
                        let built = snapshot::create(&code, base, |isolate| {
                            isolate.set_slot(Arc::clone(runtime));
                            *lock(&block.building) = Some(isolate.thread_safe_handle());
                        });
                        lock(&block.building).take();
                        built
                    };
                    block
                        .term
                        .end(dispatch)
                        .map_or(built, |halt| Err(halted(halt)))
                }
                Depth::Nested => Err(EngineError::Internal(
                    "snapshot inside a callback".to_owned(),
                )),
            };
            let served = respond(block, response);
            sys::release_free_memory();
            return served;
        }
        Some(ToV8::Request(request)) => request,
        // No callback is outstanding here, so there is nothing to reply to.
        Some(ToV8::Reply(_)) => return Served::Next,
        None => return Served::Stop { disposed: false },
    };
    let release = match &request {
        Request::Eval { source, .. } => large(source),
        Request::LowMemoryNotification { .. } => true,
        _ => false,
    };
    let response = {
        v8::scope_with_context!(let scope, isolate, &runtime.context);
        handle(scope, runtime, request)
    };
    let served = respond(block, response);
    // After the response, so the caller does not wait for it.
    if release {
        sys::release_free_memory();
    }
    served
}

// A bundle, not a render: what parsing it frees is worth handing back.
const LARGE_SOURCE: usize = 1 << 20;

const fn large(source: &JsText) -> bool {
    match source {
        JsText::Utf8(bytes) | JsText::Latin1(bytes) => bytes.len() >= LARGE_SOURCE,
        JsText::Utf16(units) => units.len() >= LARGE_SOURCE / 2,
    }
}

fn respond(block: &ControlBlock, response: Result<Vec<u8>, EngineError>) -> Served {
    if block.channel.respond(response).is_ok() {
        Served::Next
    } else {
        Served::Stop { disposed: false }
    }
}

enum Received {
    Message(Option<ToV8<crate::control::Wire>>),
    Idle,
}

fn receive_within(block: &ControlBlock, idle: Duration) -> Received {
    let deadline = std::time::Instant::now().checked_add(idle);
    spin_until(IDLE_SPIN.min(idle), || block.channel.ready(Side::V8));
    loop {
        // As mini_racer's runner: once the deadline has passed it collects
        // before it looks for a request, which then waits in the channel.
        let left = deadline.map(|at| at.saturating_duration_since(std::time::Instant::now()));
        if left.is_some_and(|left| left.is_zero()) {
            return Received::Idle;
        }
        match block.channel.recv_v8() {
            Ok(Some(message)) => return Received::Message(Some(message)),
            Ok(None) => block.channel.park(Side::V8, left),
            Err(_) => return Received::Message(None),
        }
    }
}

fn receive(block: &ControlBlock) -> Option<ToV8<crate::control::Wire>> {
    spin_until(IDLE_SPIN, || block.channel.ready(Side::V8));
    loop {
        match block.channel.recv_v8() {
            Ok(Some(message)) => return Some(message),
            Ok(None) => block.channel.park(Side::V8, None),
            Err(_) => return None,
        }
    }
}

fn handle(
    scope: &mut v8::PinScope<'_, '_>,
    runtime: &Runtime,
    request: Request,
) -> Result<Vec<u8>, EngineError> {
    match request {
        Request::Init { depth } => match depth {
            Depth::Outermost(dispatch) => ended(runtime, dispatch),
            Depth::Nested => Err(EngineError::Internal("setup inside a callback".to_owned())),
        },
        Request::Eval {
            depth: Depth::Nested,
            awaited: true,
            ..
        }
        | Request::Call {
            depth: Depth::Nested,
            awaited: true,
            ..
        } => refuse_nested_await(scope, runtime),
        Request::Eval {
            filename,
            source,
            depth,
            awaited,
        } => script(scope, runtime, depth, DispatchKind::Script, |tc| {
            evaluate(tc, runtime, (&filename, &source), (depth, awaited))
        }),
        Request::Call {
            request,
            depth,
            awaited,
        } => script(scope, runtime, depth, DispatchKind::Script, |tc| {
            invoke(tc, runtime, &request, (depth, awaited))
        }),
        Request::Attach { request, depth } => {
            script(scope, runtime, depth, DispatchKind::Maintenance, |tc| {
                attach(tc, &request)
            })
        }
        // A collection for a context about to be torn down is wasted.
        Request::LowMemoryNotification { depth } => {
            if !runtime.block.term.is_disposing() {
                scope.low_memory_notification();
            }
            match depth {
                Depth::Outermost(dispatch) => ended(runtime, dispatch),
                Depth::Nested => Ok(Vec::new()),
            }
        }
        // mini_racer leaves a termination this meets for the dispatch around
        // it, or for the next script.
        Request::MicrotaskCheckpoint { depth } => {
            script(scope, runtime, depth, DispatchKind::Maintenance, |tc| {
                tc.perform_microtask_checkpoint();
                Ok(Vec::new())
            })
        }
        Request::HeapStats { depth } => {
            script(scope, runtime, depth, DispatchKind::Maintenance, |tc| {
                Ok(heap_stats(tc))
            })
        }
        Request::HeapSnapshot { depth } => {
            script(scope, runtime, depth, DispatchKind::Maintenance, |tc| {
                Ok(heap_snapshot(tc))
            })
        }
        Request::PumpMessageLoop { depth } => {
            script(scope, runtime, depth, DispatchKind::Pump, |tc| {
                Ok(vec![u8::from(pump(tc, runtime))])
            })
        }
        // The extension refuses a dispose from inside a callback before it
        // gets here.
        Request::Dispose => Err(EngineError::DisposeBusy),
        // Snapshots are built on a context of their own, never nested.
        Request::Snapshot { .. } => Err(EngineError::Internal(
            "snapshot inside a callback".to_owned(),
        )),
    }
}

// A maintenance dispatch reports only whether a dispose caught it.
fn ended(runtime: &Runtime, dispatch: DispatchId) -> Result<Vec<u8>, EngineError> {
    match runtime.block.term.end(dispatch) {
        Some(_) => Err(EngineError::Disposed),
        None => Ok(Vec::new()),
    }
}

fn halted(halt: Halt) -> EngineError {
    match halt {
        Halt::Terminated(cause) => EngineError::from(JsError::terminated(cause)),
        // mini_racer cancels the wait of a dispatch a dispose catches in
        // flight, which raises its disposed error.
        Halt::Disposed => EngineError::Disposed,
    }
}

// Runs a dispatch: nothing starts once it is halted. A script takes its
// verdict before serializing its result (`verdict`) and stays armed through
// it, as serializing runs getters: a cause that stops the serializing is
// reported, and one that lands without stopping it leaves the reply. A nested
// dispatch reports a halt but leaves its cause to the dispatch it runs in. An
// exception that escapes as the one a raising callback threw takes
// precedence, as in mini_racer.
fn script(
    scope: &mut v8::PinScope<'_, '_>,
    runtime: &Runtime,
    depth: Depth,
    kind: DispatchKind,
    run: impl FnOnce(&mut Tc<'_, '_, '_, '_>) -> Result<Vec<u8>, EngineError>,
) -> Result<Vec<u8>, EngineError> {
    let term = &runtime.block.term;
    v8::tc_scope!(let tc, scope);
    tc.set_verbose(runtime.block.config.verbose_exceptions);
    let result = if let Err(error) = arm_timeout(runtime, depth, kind) {
        Err(error)
    } else if term.refuses(kind) {
        Err(EngineError::Internal("halted without a cause".to_owned()))
    } else {
        run(tc)
    };
    let since = term.mark();
    // mini_racer's maintenance never cancels a termination it meets, so the
    // interrupt is used up while the cause waits for the next script. Inside
    // a callback, the termination caught here would otherwise unwind the
    // script around it, which in mini_racer runs on and reports the cause
    // when it ends.
    if kind == DispatchKind::Maintenance
        && tc.has_terminated()
        && term.spend(since, matches!(depth, Depth::Nested))
    {
        // Lifted, the termination this caught must not be rethrown as the
        // scope closes.
        tc.reset();
    }
    let raised = result.is_err() && kind == DispatchKind::Script && escaped(tc, runtime);
    let halt = match depth {
        Depth::Outermost(dispatch) if result.is_ok() => term.finish(dispatch),
        // mini_racer replies to a terminated script with its termination
        // cancelled, and V8 runs the microtasks the script left then, still
        // within the dispatch. A cause V8 acts on there is carried, spent, to
        // the next script; one still pending when they finish fails the
        // reply, and mini_racer's retry reports it with this script. When the
        // script's own timeout ended it, mini_racer runs them with no limit;
        // the timeout bounds them again here.
        Depth::Outermost(dispatch) if kind == DispatchKind::Script && tc.has_terminated() => {
            let verdict = term.settle(dispatch);
            let drains = verdict.is_some_and(|halt| halt != Halt::Disposed)
                && rearm_timeout(runtime, dispatch).is_ok();
            if drains {
                tc.reset();
                tc.perform_microtask_checkpoint();
            }
            if drains && (tc.has_terminated() || tc.is_execution_terminating()) {
                term.spend(term.mark(), false);
                match term.finish(dispatch) {
                    Some(Halt::Disposed) => Some(Halt::Disposed),
                    _ => verdict,
                }
            } else if drains {
                term.end_drained(dispatch).or(verdict)
            } else {
                match term.end(dispatch) {
                    Some(Halt::Disposed) => Some(Halt::Disposed),
                    _ => verdict,
                }
            }
        }
        Depth::Outermost(dispatch) => term.end(dispatch),
        // Its verdict came before it serialized, but a dispose still cancels
        // the wait for it, as in mini_racer.
        Depth::Nested if result.is_ok() && kind == DispatchKind::Script => {
            term.is_disposing().then_some(Halt::Disposed)
        }
        Depth::Nested => {
            let halt = term.check(kind);
            if kind == DispatchKind::Script && matches!(halt, Some(Halt::Terminated(_))) {
                term.renew();
            }
            halt
        }
    };
    if raised {
        return Err(EngineError::from(JsError {
            kind: JsErrorKind::HostException,
            message: String::new(),
        }));
    }
    halt.map_or(result, |halt| Err(halted(halt)))
}

// mini_racer starts a script's watchdog on its V8 thread as it takes the
// request, so time spent waiting for the runner, such as behind a collection
// after `ensure_gc_after_idle`, never counts against the timeout. Setup and
// maintenance are never timed.
fn arm_timeout(runtime: &Runtime, depth: Depth, kind: DispatchKind) -> Result<(), EngineError> {
    match (depth, kind, runtime.block.config.timeout) {
        (Depth::Outermost(dispatch), DispatchKind::Script, Some(timeout)) => {
            watchdog::schedule(timeout, &runtime.block, dispatch)
        }
        _ => Ok(()),
    }
}

fn rearm_timeout(runtime: &Runtime, dispatch: DispatchId) -> Result<(), EngineError> {
    runtime.block.config.timeout.map_or(Ok(()), |timeout| {
        watchdog::rearm_if_fired(timeout, &runtime.block, dispatch)
    })
}

fn escaped(tc: &Tc<'_, '_, '_, '_>, runtime: &Runtime) -> bool {
    let Some(exception) = tc.exception() else {
        return false;
    };
    lock(&runtime.raised)
        .as_ref()
        .is_some_and(|raised| v8::Local::new(tc, raised).same_value(exception))
}

fn evaluate(
    tc: &mut Tc<'_, '_, '_, '_>,
    runtime: &Runtime,
    (filename, source): (&JsText, &JsText),
    (depth, awaited): (Depth, bool),
) -> Result<Vec<u8>, EngineError> {
    let too_large = || EngineError::Internal("source too large".to_owned());
    let source = string(tc, source).ok_or_else(too_large)?;
    let name = string(tc, filename).ok_or_else(too_large)?;
    let origin = serialize::origin(tc, name.into());
    let script = v8::Script::compile(tc, source, Some(&origin))
        .ok_or_else(|| failure(tc, JsErrorKind::Parse))?;
    let value = script
        .run(tc)
        .ok_or_else(|| failure(tc, JsErrorKind::Runtime))?;
    let value = if awaited {
        await_promise(tc, runtime, value)?
    } else {
        value
    };
    let value = serialize::sanitize(tc, value);
    verdict(runtime, depth)?;
    serialize::reply(tc, &runtime.filter, value).ok_or_else(|| failure(tc, JsErrorKind::Runtime))
}

// mini_racer's last termination check, before a script's result is
// serialized. A nested script reads the causes and leaves them to the
// dispatch around it.
fn verdict(runtime: &Runtime, depth: Depth) -> Result<(), EngineError> {
    let term = &runtime.block.term;
    let halt = match depth {
        Depth::Outermost(dispatch) => term.settle(dispatch),
        Depth::Nested => term.check(DispatchKind::Script),
    };
    halt.map_or(Ok(()), |halt| Err(halted(halt)))
}

// V8 runs no microtask checkpoint inside another, so an await nested in a
// callback could never settle; mini_racer refuses one before running anything.
// With a stop or timeout pending, it reports the refusal as terminated, and the
// refusal's text stays the message.
fn refuse_nested_await(
    scope: &mut v8::PinScope<'_, '_>,
    runtime: &Runtime,
) -> Result<Vec<u8>, EngineError> {
    v8::tc_scope!(let tc, scope);
    tc.set_verbose(runtime.block.config.verbose_exceptions);
    let failed = |tc: &Tc<'_, '_, '_, '_>| failure(tc, JsErrorKind::Runtime);
    let message = v8::String::new(tc, "nested call_await/eval_await is not supported")
        .ok_or_else(|| failed(tc))?;
    let exception = v8::Exception::error(tc, message);
    tc.throw_exception(exception);
    let refused = failed(tc);
    let halt = runtime.block.term.check(DispatchKind::Script);
    if matches!(halt, Some(Halt::Terminated(_))) {
        runtime.block.term.renew();
    }
    let halt = halt.map(halted);
    Err(match (halt, refused) {
        (Some(EngineError::Js(halt)), EngineError::Js(refused)) => EngineError::from(JsError {
            kind: halt.kind,
            message: refused.message,
        }),
        (Some(disposed @ EngineError::Disposed), _) => disposed,
        (_, refused) => refused,
    })
}

// mini_racer's await: run the microtasks, then look at the promise. While it
// is pending, run due tasks one at a time, sleeping until one is due, until
// the promise settles or the dispatch is halted.
fn await_promise<'s>(
    tc: &mut Tc<'s, '_, '_, '_>,
    runtime: &Runtime,
    value: v8::Local<'s, v8::Value>,
) -> Result<v8::Local<'s, v8::Value>, EngineError> {
    let Ok(promise) = v8::Local::<v8::Promise>::try_from(value) else {
        return Ok(value);
    };
    loop {
        tc.perform_microtask_checkpoint();
        match promise.state() {
            v8::PromiseState::Fulfilled => return Ok(promise.result(tc)),
            v8::PromiseState::Rejected => {
                let reason = promise.result(tc);
                tc.throw_exception(reason);
                return Err(failure(tc, JsErrorKind::Runtime));
            }
            v8::PromiseState::Pending => {}
        }
        if tc.is_execution_terminating() || runtime.block.term.check(DispatchKind::Script).is_some()
        {
            return Err(EngineError::from(JsError::terminated(
                TerminationCause::Stop,
            )));
        }
        match runtime.block.tasks.pop(false) {
            Some(task) => run_task(runtime, task),
            None => wait_for_task(runtime),
        }
    }
}

// Waiting, the runner holds its isolate but runs nothing in it, so a fork need
// not wait for it.
fn wait_for_task(runtime: &Runtime) {
    if runtime.released {
        runtime.block.phase.set(Phase::Parked);
    }
    runtime.block.tasks.wait();
    if runtime.released {
        runtime.generation.resume(&runtime.block);
    }
}

fn run_task(runtime: &Runtime, task: v8::Task) {
    runtime.running.fetch_add(1, Ordering::Relaxed);
    let _running = Running(&runtime.running);
    task.run();
}

// mini_racer's heap statistics, in its order: the numbers V8 reports, which
// it hands JavaScript as doubles.
#[expect(
    clippy::cast_precision_loss,
    reason = "mini_racer passes each statistic to Ruby as a V8 number"
)]
fn heap_stats(isolate: &mut v8::Isolate) -> Vec<u8> {
    let stats = isolate.get_heap_statistics();
    let named = [
        ("total_heap_size", stats.total_heap_size()),
        (
            "total_heap_size_executable",
            stats.total_heap_size_executable(),
        ),
        ("total_physical_size", stats.total_physical_size()),
        ("total_available_size", stats.total_available_size()),
        (
            "total_global_handles_size",
            stats.total_global_handles_size(),
        ),
        ("used_global_handles_size", stats.used_global_handles_size()),
        ("used_heap_size", stats.used_heap_size()),
        ("heap_size_limit", stats.heap_size_limit()),
        ("malloced_memory", stats.malloced_memory()),
        ("external_memory", stats.external_memory()),
        ("peak_malloced_memory", stats.peak_malloced_memory()),
        (
            "number_of_native_contexts",
            stats.number_of_native_contexts(),
        ),
        (
            "number_of_detached_contexts",
            stats.number_of_detached_contexts(),
        ),
    ];
    let mut out = Vec::new();
    let mut writer = Writer::new(&mut out);
    writer.begin_object();
    for (name, value) in named {
        writer.utf8(name.as_bytes());
        writer.number(value as f64);
    }
    writer.end_object(u32::try_from(named.len()).unwrap_or(u32::MAX));
    out
}

fn heap_snapshot(isolate: &mut v8::Isolate) -> Vec<u8> {
    let mut json = Vec::new();
    isolate.take_heap_snapshot(|chunk| {
        json.extend_from_slice(chunk);
        true
    });
    json
}

// mini_racer's pump: one due task, then a microtask checkpoint if it ran and
// left nothing thrown or terminating. A pump from a running task's script
// passes over non-nestable tasks, as V8's own runner does.
fn pump(tc: &mut Tc<'_, '_, '_, '_>, runtime: &Runtime) -> bool {
    let nested = runtime.running.load(Ordering::Relaxed) > 0;
    let Some(task) = runtime.block.tasks.pop(nested) else {
        return false;
    };
    run_task(runtime, task);
    if !tc.is_execution_terminating() && !tc.has_caught() {
        tc.perform_microtask_checkpoint();
    }
    true
}

struct Reader;

impl v8::ValueDeserializerImpl for Reader {}

// `call` and `attach` requests are an array whose first element is a dotted
// path; failing to read one is an internal error in mini_racer.
fn read_request<'s>(
    tc: &Tc<'s, '_, '_, '_>,
    request: &[u8],
) -> Option<(v8::Local<'s, v8::Object>, String)> {
    let context = tc.get_current_context();
    let deserializer = v8::ValueDeserializer::new(tc, Box::new(Reader), request);
    if deserializer.read_header(context) != Some(true) {
        return None;
    }
    let request = deserializer
        .read_value(context)
        .and_then(|request| request.to_object(tc))?;
    let path = request
        .get_index(tc, 0)
        .and_then(|name| name.to_string(tc))?
        .to_rust_string_lossy(tc);
    Some((request, path))
}

// mini_racer's order: everything from the path walk on fails as a runtime
// error. Each dotted segment but the last becomes an object, and the function
// is called on its parent.
fn invoke(
    tc: &mut Tc<'_, '_, '_, '_>,
    runtime: &Runtime,
    request: &[u8],
    (depth, awaited): (Depth, bool),
) -> Result<Vec<u8>, EngineError> {
    let context = tc.get_current_context();
    let failed = |tc: &Tc<'_, '_, '_, '_>| failure(tc, JsErrorKind::Runtime);
    let (request, path) =
        read_request(tc, request).ok_or_else(|| failure(tc, JsErrorKind::Internal))?;
    let mut receiver = context.global(tc);
    let mut keys = path.split('.').peekable();
    let mut function = None;
    while let Some(key) = keys.next() {
        let key = v8::String::new(tc, key).ok_or_else(|| failed(tc))?;
        let value = receiver.get(tc, key.into()).ok_or_else(|| failed(tc))?;
        if keys.peek().is_none() {
            function = Some(value);
        } else {
            receiver = value.to_object(tc).ok_or_else(|| failed(tc))?;
        }
    }
    let Some(Ok(function)) = function.map(v8::Local::<v8::Function>::try_from) else {
        let message = v8::String::new(tc, "not a function").ok_or_else(|| failed(tc))?;
        let exception = v8::Exception::type_error(tc, message);
        tc.throw_exception(exception);
        return Err(failed(tc));
    };
    let count = v8::Local::<v8::Array>::try_from(request).map_or(0, |array| array.length());
    let mut args = Vec::new();
    for index in 1..count {
        args.push(request.get_index(tc, index).ok_or_else(|| failed(tc))?);
    }
    let value = function
        .call(tc, receiver.into(), &args)
        .ok_or_else(|| failed(tc))?;
    let value = if awaited {
        await_promise(tc, runtime, value)?
    } else {
        value
    };
    let value = serialize::sanitize(tc, value);
    verdict(runtime, depth)?;
    serialize::reply(tc, &runtime.filter, value).ok_or_else(|| failed(tc))
}

// mini_racer's attach: any failure is an internal error, even under a pending
// termination, which attach never checks for; the message is then V8's
// termination exception, "null". Each dotted segment but the last that is not
// an object or function becomes a new object, and the callback is set on the
// last.
fn attach(tc: &Tc<'_, '_, '_, '_>, request: &[u8]) -> Result<Vec<u8>, EngineError> {
    let context = tc.get_current_context();
    let internal = |tc: &Tc<'_, '_, '_, '_>| described(tc, JsErrorKind::Internal);
    let (request, path) = read_request(tc, request).ok_or_else(|| internal(tc))?;
    let id = request
        .get_index(tc, 1)
        .filter(|id| id.is_int32())
        .ok_or_else(|| internal(tc))?;
    let mut object = context.global(tc);
    let mut keys = path.split('.').peekable();
    while let Some(key) = keys.next() {
        let key = v8::String::new(tc, key).ok_or_else(|| internal(tc))?;
        if keys.peek().is_none() {
            let function = v8::Function::builder(trampoline)
                .data(id)
                .build(tc)
                .ok_or_else(|| internal(tc))?;
            if object.set(tc, key.into(), function.into()) != Some(true) {
                return Err(internal(tc));
            }
            break;
        }
        let value = object.get(tc, key.into()).ok_or_else(|| internal(tc))?;
        object = if let Ok(inner) = v8::Local::<v8::Object>::try_from(value) {
            inner
        } else {
            let inner = v8::Object::new(tc);
            if object.set(tc, key.into(), inner.into()) != Some(true) {
                return Err(internal(tc));
            }
            inner
        };
    }
    Ok(Vec::new())
}

// An attached function: its arguments go to Ruby with its id, and requests
// Ruby makes meanwhile are served here until the reply comes. A panic closes
// the channel, as one in the runner loop does.
#[expect(
    clippy::needless_pass_by_value,
    reason = "the signature V8 calls a function callback with"
)]
fn trampoline<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    rv: v8::ReturnValue<'s, v8::Value>,
) {
    let Some(runtime) = scope.get_slot::<Arc<Runtime>>().cloned() else {
        return;
    };
    let outcome = catch_unwind(AssertUnwindSafe(|| callback(scope, &runtime, &args, rv)));
    if outcome.is_err() {
        scope.terminate_execution();
        runtime.block.channel.close();
    }
}

fn callback<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    runtime: &Runtime,
    args: &v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let count = args.length();
    // mini_racer builds the request in the safe context, out of reach of a
    // tampered Array.prototype.
    let request = {
        let safe = v8::Local::new(scope, &runtime.filter.context);
        let scope = &mut v8::ContextScope::new(scope, safe);
        v8::Array::new(scope, count.saturating_add(1))
    };
    let mut index = 0;
    for position in 0..count {
        let value = serialize::sanitize(scope, args.get(position));
        if request.set_index(scope, index, value).is_none() {
            return;
        }
        index = index.saturating_add(1);
    }
    if request.set_index(scope, index, args.data()).is_none() {
        return;
    }
    let Some(bytes) = serialize::write(scope, request.into()) else {
        return;
    };
    if runtime.block.channel.callback(bytes).is_err() {
        scope.terminate_execution();
        return;
    }
    loop {
        // Waiting for Ruby, V8 is still: a fork need not wait for this.
        if runtime.released {
            runtime.block.phase.set(Phase::Parked);
        }
        let message = receive(&runtime.block);
        if runtime.released {
            runtime.generation.resume(&runtime.block);
        }
        let reply = match message {
            Some(ToV8::Reply(reply)) => reply,
            Some(ToV8::Request(request)) => {
                // Each nested request gets its own handle scope, so its values
                // are released when it responds, not when the callback returns.
                let response = {
                    v8::scope_with_context!(let scope, scope, &runtime.context);
                    handle(scope, runtime, request)
                };
                if runtime.block.channel.respond(response).is_err() {
                    scope.terminate_execution();
                    return;
                }
                continue;
            }
            None => {
                scope.terminate_execution();
                return;
            }
        };
        // A nested dispatch that was halted leaves its termination to unwind
        // this script too, as mini_racer restores it.
        let terminating = scope.is_execution_terminating();
        match reply {
            // As mini_racer answers a cancelled callback: the throw stops the
            // script at once, and the termination takes over at V8's next
            // interrupt check. Once the script's verdict is in, mini_racer
            // only throws to the microtasks it left.
            CallbackReply::Abort => {
                if !terminating {
                    let exception = error(scope, &JsText::from("terminated"));
                    scope.throw_exception(exception);
                }
                if !runtime.block.term.is_settled() {
                    scope.terminate_execution();
                }
            }
            // mini_racer throws a raising callback's exception even with a
            // cause pending, or a nested script's termination restored:
            // escaping the script, it takes precedence, and the termination
            // takes over if the script catches it.
            CallbackReply::Raised(message) => {
                if terminating {
                    scope.cancel_terminate_execution();
                }
                let exception = error(scope, &message);
                *lock(&runtime.raised) = Some(v8::Global::new(scope, exception));
                scope.throw_exception(exception);
                if terminating {
                    scope.terminate_execution();
                }
            }
            _ if terminating => {
                scope.terminate_execution();
            }
            // As mini_racer reads the reply: a termination pending for this
            // script makes the read throw it, and the script ends here.
            CallbackReply::Value(bytes) => {
                let context = scope.get_current_context();
                let deserializer = v8::ValueDeserializer::new(scope, Box::new(Reader), &bytes);
                if deserializer.read_header(context) == Some(true)
                    && let Some(value) = deserializer.read_value(context)
                {
                    rv.set(value);
                }
            }
        }
        return;
    }
}

fn error<'s>(scope: &v8::PinScope<'s, '_>, message: &JsText) -> v8::Local<'s, v8::Value> {
    let message = string(scope, message)
        .or_else(|| v8::String::new(scope, "Ruby exception"))
        .unwrap_or_else(|| v8::String::empty(scope));
    v8::Exception::error(scope, message)
}

// mini_racer's suspend_termination: a failure under a pending termination is
// reported as the termination.
fn failure(tc: &Tc<'_, '_, '_, '_>, kind: JsErrorKind) -> EngineError {
    if tc.has_terminated() {
        return EngineError::from(JsError::terminated(TerminationCause::Stop));
    }
    described(tc, kind)
}

// The message mini_racer builds for each kind: a parse error with where it
// happened, a runtime error with its stack when the exception has one.
fn described(tc: &Tc<'_, '_, '_, '_>, kind: JsErrorKind) -> EngineError {
    let message = match kind {
        JsErrorKind::Parse => parse_message(tc),
        JsErrorKind::Runtime
        | JsErrorKind::Internal
        | JsErrorKind::Terminated(_)
        | JsErrorKind::HostException => tc
            .stack_trace()
            .and_then(|trace| trace.to_string(tc))
            .map(|trace| trace.to_rust_string_lossy(tc)),
    };
    let message = message.unwrap_or_else(|| {
        tc.exception()
            .and_then(|exception| exception.to_string(tc))
            .map_or_else(
                || "unexpected failure".to_owned(),
                |text| text.to_rust_string_lossy(tc),
            )
    });
    EngineError::from(JsError { kind, message })
}

pub fn string<'s>(
    scope: &v8::PinScope<'s, '_>,
    text: &JsText,
) -> Option<v8::Local<'s, v8::String>> {
    match text {
        JsText::Utf8(bytes) => v8::String::new_from_utf8(scope, bytes, v8::NewStringType::Normal),
        JsText::Latin1(bytes) => {
            v8::String::new_from_one_byte(scope, bytes, v8::NewStringType::Normal)
        }
        JsText::Utf16(units) => {
            v8::String::new_from_two_byte(scope, units, v8::NewStringType::Normal)
        }
    }
}

fn parse_message(tc: &Tc<'_, '_, '_, '_>) -> Option<String> {
    located(tc, " at ")
}

/// The pending exception's message and where it happened, as `mini_racer`
/// formats both, joined by `separator`.
pub fn located(tc: &Tc<'_, '_, '_, '_>, separator: &str) -> Option<String> {
    let message = tc.message()?;
    let text = message.get(tc).to_rust_string_lossy(tc);
    let name = message
        .get_script_resource_name(tc)?
        .to_string(tc)?
        .to_rust_string_lossy(tc);
    let line = message.get_line_number(tc).unwrap_or(0);
    // V8's -1 for no column comes back as usize::MAX; mini_racer prints it signed.
    let column = message.get_start_column().cast_signed();
    Some(format!("{text}{separator}{name}:{line}:{column}"))
}
