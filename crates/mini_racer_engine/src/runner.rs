use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::time::Duration;

use mini_racer_sync::{DispatchId, DispatchKind, Halt, Side, TermCell, TerminationCause, ToV8};
use v8::ValueSerializerHelper;

use crate::control::{ControlBlock, IsolateTerminator, Request};
use crate::spin::spin_until;
use crate::{EngineError, JsError, JsErrorKind};

// Spinning after each response catches a busy caller's next request without a
// futex wake on either side; an idle context then parks.
const IDLE_SPIN: Duration = Duration::from_micros(100);

struct Serializer;

impl v8::ValueSerializerImpl for Serializer {
    fn throw_data_clone_error<'s>(
        &self,
        scope: &mut v8::PinScope<'s, '_>,
        message: v8::Local<'s, v8::String>,
    ) {
        let exception = v8::Exception::error(scope, message);
        scope.throw_exception(exception);
    }
}

macro_rules! js_error {
    ($tc:expr, $kind:expr) => {{
        if $tc.has_terminated() {
            EngineError::from(JsError::terminated(TerminationCause::Stop))
        } else {
            let message = $tc.exception().map_or_else(
                || "unknown JavaScript error".to_owned(),
                |exception| exception.to_rust_string_lossy($tc),
            );
            EngineError::from(JsError {
                kind: $kind,
                message,
            })
        }
    }};
}

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
    let context = {
        v8::scope!(let scope, &mut isolate);
        let context = v8::Context::new(scope, v8::ContextOptions::default());
        v8::Global::new(scope, context)
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
            Request::Eval { source, dispatch } => {
                v8::scope_with_context!(let scope, &mut isolate, &context);
                eval(scope, &block.term, &source, dispatch)
            }
            Request::Dispose => break true,
        };
        if block.channel.respond(response).is_err() {
            break false;
        }
    };
    block.term.uninstall();
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
    source: &str,
    dispatch: DispatchId,
) -> Result<Vec<u8>, EngineError> {
    v8::tc_scope!(let tc, scope);
    let context = tc.get_current_context();
    let result = if term.check(DispatchKind::Script).is_some() {
        Err(EngineError::Internal("halted without a cause".to_owned()))
    } else {
        v8::String::new(tc, source)
            .ok_or_else(|| EngineError::Internal("source too large".to_owned()))
            .and_then(|source| {
                v8::Script::compile(tc, source, None)
                    .ok_or_else(|| js_error!(tc, JsErrorKind::Parse))
            })
            .and_then(|script| {
                script
                    .run(tc)
                    .ok_or_else(|| js_error!(tc, JsErrorKind::Runtime))
            })
            .and_then(|value| {
                let serializer = v8::ValueSerializer::new(tc, Box::new(Serializer));
                serializer.write_header();
                match serializer.write_value(context, value) {
                    Some(true) => Ok(serializer.release()),
                    _ => Err(js_error!(tc, JsErrorKind::Runtime)),
                }
            })
    };
    // Serializing runs getters, so the dispatch stays armed through it; the
    // verdict then replaces whatever the script produced.
    term.end(dispatch).map_or(result, |halt| Err(halted(halt)))
}
