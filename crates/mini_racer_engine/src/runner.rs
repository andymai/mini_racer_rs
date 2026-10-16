use std::cell::RefCell;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use mini_racer_sync::{DispatchKind, Halt, Side, TerminationCause, ToV8};
use v8::ValueDeserializerHelper;

use crate::control::{CallbackReply, ControlBlock, Depth, IsolateTerminator, JsText, Request};
use crate::serialize::{self, Filter, Tc};
use crate::snapshot;
use crate::spin::spin_until;
use crate::{EngineError, JsError, JsErrorKind};

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

// Kept in an isolate slot: the callback trampoline gets only the isolate.
struct Runtime {
    block: Arc<ControlBlock>,
    context: v8::Global<v8::Context>,
    filter: Filter,
    // The exception last thrown for a raising callback; mini_racer keeps one
    // per isolate and compares what escapes a dispatch against it.
    raised: RefCell<Option<v8::Global<v8::Value>>>,
}

pub fn run(block: &Arc<ControlBlock>) {
    let closing = Closing(block);
    let disposed = catch_unwind(AssertUnwindSafe(|| serve(block))).unwrap_or_else(|_| {
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
}

// Returns whether it stopped for an explicit dispose.
fn serve(block: &Arc<ControlBlock>) -> bool {
    let params = v8::CreateParams::default();
    let params = match &block.config.snapshot {
        Some(blob) => params.snapshot_blob(v8::StartupData::from(blob.to_vec())),
        None => params,
    };
    let mut isolate = v8::Isolate::new(params);
    let contexts = {
        v8::scope!(let scope, &mut isolate);
        serialize::contexts(scope)
    };
    let Some((context, filter)) = contexts else {
        if receive(block).is_some() {
            let _ = block.channel.respond(Err(EngineError::Internal(
                "context setup failed".to_owned(),
            )));
        }
        return false;
    };
    let runtime = Rc::new(Runtime {
        block: Arc::clone(block),
        context,
        filter,
        raised: RefCell::default(),
    });
    isolate.set_slot(Rc::clone(&runtime));
    // Installed only once the context exists, so nothing terminates its setup.
    block
        .term
        .install(IsolateTerminator(isolate.thread_safe_handle()));
    let disposed = loop {
        let request = match receive(block) {
            Some(ToV8::Request(Request::Dispose)) => break true,
            // The snapshot's own isolate is entered with none of this
            // runner's scopes open.
            Some(ToV8::Request(Request::Snapshot { code, base, depth })) => {
                let built = snapshot::create(&code, base);
                let response = match depth {
                    Depth::Outermost(dispatch) => block
                        .term
                        .end(dispatch)
                        .map_or(built, |halt| Err(halted(halt))),
                    Depth::Nested => Err(EngineError::Internal(
                        "snapshot inside a callback".to_owned(),
                    )),
                };
                if block.channel.respond(response).is_err() {
                    break false;
                }
                continue;
            }
            Some(ToV8::Request(request)) => request,
            // No callback is outstanding here, so there is nothing to reply to.
            Some(ToV8::Reply(_)) => continue,
            None => break false,
        };
        let response = {
            v8::scope_with_context!(let scope, &mut isolate, &runtime.context);
            handle(scope, &runtime, request)
        };
        if block.channel.respond(response).is_err() {
            break false;
        }
    };
    block.term.uninstall();
    drop(isolate.remove_slot::<Rc<Runtime>>());
    drop(runtime);
    drop(isolate);
    disposed
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
            Depth::Outermost(dispatch) => match runtime.block.term.end(dispatch) {
                Some(_) => Err(EngineError::Disposed),
                None => Ok(Vec::new()),
            },
            Depth::Nested => Err(EngineError::Internal("setup inside a callback".to_owned())),
        },
        Request::Eval {
            filename,
            source,
            depth,
        } => script(scope, runtime, depth, DispatchKind::Script, |tc| {
            evaluate(tc, &runtime.filter, &filename, &source)
        }),
        Request::Call { request, depth } => {
            script(scope, runtime, depth, DispatchKind::Script, |tc| {
                invoke(tc, &runtime.filter, &request)
            })
        }
        Request::Attach { request, depth } => {
            script(scope, runtime, depth, DispatchKind::Maintenance, |tc| {
                attach(tc, &request)
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

fn halted(halt: Halt) -> EngineError {
    match halt {
        Halt::Terminated(cause) => EngineError::from(JsError::terminated(cause)),
        // mini_racer terminates a dispatch caught by dispose exactly as stop does.
        Halt::Disposed => EngineError::from(JsError::terminated(TerminationCause::Stop)),
    }
}

// Runs a dispatch: nothing starts once it is halted, and serializing runs
// getters, so an outermost dispatch stays armed through `run`; the verdict then
// replaces whatever the script produced. A nested dispatch reports a halt but
// leaves its cause to the dispatch it runs in. An exception that escapes as
// the one a raising callback threw takes precedence, as in mini_racer.
fn script(
    scope: &mut v8::PinScope<'_, '_>,
    runtime: &Runtime,
    depth: Depth,
    kind: DispatchKind,
    run: impl FnOnce(&mut Tc<'_, '_, '_, '_>) -> Result<Vec<u8>, EngineError>,
) -> Result<Vec<u8>, EngineError> {
    let term = &runtime.block.term;
    v8::tc_scope!(let tc, scope);
    let result = if term.check(kind).is_some() {
        Err(EngineError::Internal("halted without a cause".to_owned()))
    } else {
        run(tc)
    };
    let raised = result.is_err() && kind == DispatchKind::Script && escaped(tc, runtime);
    let halt = match depth {
        Depth::Outermost(dispatch) => term.end(dispatch),
        Depth::Nested => term.check(kind),
    };
    if raised {
        return Err(EngineError::from(JsError {
            kind: JsErrorKind::HostException,
            message: String::new(),
        }));
    }
    halt.map_or(result, |halt| Err(halted(halt)))
}

fn escaped(tc: &Tc<'_, '_, '_, '_>, runtime: &Runtime) -> bool {
    let Some(exception) = tc.exception() else {
        return false;
    };
    runtime
        .raised
        .borrow()
        .as_ref()
        .is_some_and(|raised| v8::Local::new(tc, raised).same_value(exception))
}

fn evaluate(
    tc: &mut Tc<'_, '_, '_, '_>,
    filter: &Filter,
    filename: &JsText,
    source: &JsText,
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
    let value = serialize::sanitize(tc, value);
    serialize::reply(tc, filter, value).ok_or_else(|| failure(tc, JsErrorKind::Runtime))
}

struct Reader;

impl v8::ValueDeserializerImpl for Reader {}

// `call` and `attach` requests are an array whose first element is a dotted
// path; failing to read one is an internal error in mini_racer.
fn read_request<'s>(
    tc: &Tc<'s, '_, '_, '_>,
    request: &[u8],
) -> Result<(v8::Local<'s, v8::Object>, String), EngineError> {
    let context = tc.get_current_context();
    let internal = |tc: &Tc<'_, '_, '_, '_>| failure(tc, JsErrorKind::Internal);
    let deserializer = v8::ValueDeserializer::new(tc, Box::new(Reader), request);
    if deserializer.read_header(context) != Some(true) {
        return Err(internal(tc));
    }
    let request = deserializer
        .read_value(context)
        .and_then(|request| request.to_object(tc))
        .ok_or_else(|| internal(tc))?;
    let path = request
        .get_index(tc, 0)
        .and_then(|name| name.to_string(tc))
        .ok_or_else(|| internal(tc))?
        .to_rust_string_lossy(tc);
    Ok((request, path))
}

// mini_racer's order: everything from the path walk on fails as a runtime
// error. Each dotted segment but the last becomes an object, and the function
// is called on its parent.
fn invoke(
    tc: &mut Tc<'_, '_, '_, '_>,
    filter: &Filter,
    request: &[u8],
) -> Result<Vec<u8>, EngineError> {
    let context = tc.get_current_context();
    let runtime = |tc: &Tc<'_, '_, '_, '_>| failure(tc, JsErrorKind::Runtime);
    let (request, path) = read_request(tc, request)?;
    let mut receiver = context.global(tc);
    let mut keys = path.split('.').peekable();
    let mut function = None;
    while let Some(key) = keys.next() {
        let key = v8::String::new(tc, key).ok_or_else(|| runtime(tc))?;
        let value = receiver.get(tc, key.into()).ok_or_else(|| runtime(tc))?;
        if keys.peek().is_none() {
            function = Some(value);
        } else {
            receiver = value.to_object(tc).ok_or_else(|| runtime(tc))?;
        }
    }
    let Some(Ok(function)) = function.map(v8::Local::<v8::Function>::try_from) else {
        let message = v8::String::new(tc, "not a function").ok_or_else(|| runtime(tc))?;
        let exception = v8::Exception::type_error(tc, message);
        tc.throw_exception(exception);
        return Err(runtime(tc));
    };
    let count = v8::Local::<v8::Array>::try_from(request).map_or(0, |array| array.length());
    let mut args = Vec::new();
    for index in 1..count {
        args.push(request.get_index(tc, index).ok_or_else(|| runtime(tc))?);
    }
    let value = function
        .call(tc, receiver.into(), &args)
        .ok_or_else(|| runtime(tc))?;
    let value = serialize::sanitize(tc, value);
    serialize::reply(tc, filter, value).ok_or_else(|| runtime(tc))
}

// mini_racer's attach: any failure is an internal error. Each dotted segment
// but the last that is not an object or function becomes a new object, and
// the callback is set on the last.
fn attach(tc: &Tc<'_, '_, '_, '_>, request: &[u8]) -> Result<Vec<u8>, EngineError> {
    let context = tc.get_current_context();
    let internal = |tc: &Tc<'_, '_, '_, '_>| failure(tc, JsErrorKind::Internal);
    let (request, path) = read_request(tc, request)?;
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
    let Some(runtime) = scope.get_slot::<Rc<Runtime>>().cloned() else {
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
        let reply = match receive(&runtime.block) {
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
        let terminating = scope.is_execution_terminating();
        // A nested dispatch that was halted leaves its termination to unwind
        // this script too. Throwing would replace it with a catchable error,
        // so the termination is renewed instead, as mini_racer restores it.
        let halted = terminating || runtime.block.term.check(DispatchKind::Script).is_some();
        match reply {
            // As mini_racer answers a cancelled callback: the throw stops the
            // script at once, and the termination takes over at V8's next
            // interrupt check.
            CallbackReply::Abort => {
                if !terminating {
                    let exception = error(scope, &JsText::from("terminated"));
                    scope.throw_exception(exception);
                }
                scope.terminate_execution();
            }
            _ if halted => {
                scope.terminate_execution();
            }
            CallbackReply::Value(bytes) => {
                let context = scope.get_current_context();
                let deserializer = v8::ValueDeserializer::new(scope, Box::new(Reader), &bytes);
                if deserializer.read_header(context) == Some(true)
                    && let Some(value) = deserializer.read_value(context)
                {
                    rv.set(value);
                }
            }
            CallbackReply::Raised(message) => {
                let exception = error(scope, &message);
                runtime
                    .raised
                    .replace(Some(v8::Global::new(scope, exception)));
                scope.throw_exception(exception);
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

// The message mini_racer builds for each kind: a parse error with where it
// happened, a runtime error with its stack when the exception has one.
fn failure(tc: &Tc<'_, '_, '_, '_>, kind: JsErrorKind) -> EngineError {
    if tc.has_terminated() {
        return EngineError::from(JsError::terminated(TerminationCause::Stop));
    }
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

fn string<'s>(scope: &v8::PinScope<'s, '_>, text: &JsText) -> Option<v8::Local<'s, v8::String>> {
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
    let message = tc.message()?;
    let text = message.get(tc).to_rust_string_lossy(tc);
    let name = message
        .get_script_resource_name(tc)?
        .to_string(tc)?
        .to_rust_string_lossy(tc);
    let line = message.get_line_number(tc).unwrap_or(0);
    // V8's -1 for no column comes back as usize::MAX; mini_racer prints it signed.
    let column = message.get_start_column().cast_signed();
    Some(format!("{text} at {name}:{line}:{column}"))
}
