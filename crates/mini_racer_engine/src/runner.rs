use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::time::Duration;

use mini_racer_sync::{DispatchId, DispatchKind, Halt, Side, TermCell, TerminationCause, ToV8};
use v8::ValueDeserializerHelper;

use crate::control::{ControlBlock, IsolateTerminator, JsText, Request};
use crate::serialize::{self, Filter, Tc};
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
fn serve(block: &ControlBlock) -> bool {
    let mut isolate = v8::Isolate::new(v8::CreateParams::default());
    let contexts = {
        v8::scope!(let scope, &mut isolate);
        serialize::contexts(scope)
    };
    let Some((context, filter)) = contexts else {
        if next(block).is_some() {
            let _ = block.channel.respond(Err(EngineError::Internal(
                "context setup failed".to_owned(),
            )));
        }
        return false;
    };
    // Installed only once the context exists, so nothing terminates its setup.
    block
        .term
        .install(IsolateTerminator(isolate.thread_safe_handle()));
    let disposed = loop {
        let Some(request) = next(block) else {
            break false;
        };
        let response = match request {
            Request::Init { dispatch } => match block.term.end(dispatch) {
                Some(_) => Err(EngineError::Disposed),
                None => Ok(Vec::new()),
            },
            Request::Eval {
                filename,
                source,
                dispatch,
            } => {
                v8::scope_with_context!(let scope, &mut isolate, &context);
                eval(scope, &block.term, &filter, &filename, &source, dispatch)
            }
            Request::Call { request, dispatch } => {
                v8::scope_with_context!(let scope, &mut isolate, &context);
                call(scope, &block.term, &filter, &request, dispatch)
            }
            Request::Dispose => break true,
        };
        if block.channel.respond(response).is_err() {
            break false;
        }
    };
    block.term.uninstall();
    drop(filter);
    drop(context);
    drop(isolate);
    disposed
}

fn next(block: &ControlBlock) -> Option<Request> {
    spin_until(IDLE_SPIN, || block.channel.ready(Side::V8));
    loop {
        match block.channel.recv_v8() {
            Ok(Some(ToV8::Request(request))) => return Some(request),
            Ok(Some(ToV8::Reply(reply))) => match reply {},
            Ok(None) => block.channel.park(Side::V8, None),
            Err(_) => return None,
        }
    }
}

fn halted(halt: Halt) -> EngineError {
    match halt {
        Halt::Terminated(cause) => EngineError::from(JsError::terminated(cause)),
        // mini_racer terminates a dispatch caught by dispose exactly as stop does.
        Halt::Disposed => EngineError::from(JsError::terminated(TerminationCause::Stop)),
    }
}

fn eval(
    scope: &mut v8::PinScope<'_, '_>,
    term: &TermCell<IsolateTerminator>,
    filter: &Filter,
    filename: &JsText,
    source: &JsText,
    dispatch: DispatchId,
) -> Result<Vec<u8>, EngineError> {
    v8::tc_scope!(let tc, scope);
    let result = if term.check(DispatchKind::Script).is_some() {
        Err(EngineError::Internal("halted without a cause".to_owned()))
    } else {
        evaluate(tc, filter, filename, source)
    };
    // Serializing runs getters, so the dispatch stays armed through it; the
    // verdict then replaces whatever the script produced.
    term.end(dispatch).map_or(result, |halt| Err(halted(halt)))
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

fn call(
    scope: &mut v8::PinScope<'_, '_>,
    term: &TermCell<IsolateTerminator>,
    filter: &Filter,
    request: &[u8],
    dispatch: DispatchId,
) -> Result<Vec<u8>, EngineError> {
    v8::tc_scope!(let tc, scope);
    let result = if term.check(DispatchKind::Script).is_some() {
        Err(EngineError::Internal("halted without a cause".to_owned()))
    } else {
        invoke(tc, filter, request)
    };
    term.end(dispatch).map_or(result, |halt| Err(halted(halt)))
}

struct Reader;

impl v8::ValueDeserializerImpl for Reader {}

// mini_racer's order: reading the request fails as an internal error, and
// everything from the path walk on as a runtime error. Each dotted segment
// but the last becomes an object, and the function is called on its parent.
fn invoke(
    tc: &mut Tc<'_, '_, '_, '_>,
    filter: &Filter,
    request: &[u8],
) -> Result<Vec<u8>, EngineError> {
    let context = tc.get_current_context();
    let internal = |tc: &Tc<'_, '_, '_, '_>| failure(tc, JsErrorKind::Internal);
    let runtime = |tc: &Tc<'_, '_, '_, '_>| failure(tc, JsErrorKind::Runtime);
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

// The message mini_racer builds for each kind: a parse error with where it
// happened, a runtime error with its stack when the exception has one.
fn failure(tc: &Tc<'_, '_, '_, '_>, kind: JsErrorKind) -> EngineError {
    if tc.has_terminated() {
        return EngineError::from(JsError::terminated(TerminationCause::Stop));
    }
    let message = match kind {
        JsErrorKind::Parse => parse_message(tc),
        JsErrorKind::Runtime | JsErrorKind::Internal | JsErrorKind::Terminated(_) => tc
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
