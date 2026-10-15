use std::sync::mpsc::Receiver;

use v8::ValueSerializerHelper;

use crate::context::Request;
use crate::{EngineError, JsError, JsErrorKind};

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
        let message = $tc.exception().map_or_else(
            || "unknown JavaScript error".to_owned(),
            |exception| exception.to_rust_string_lossy($tc),
        );
        EngineError::from(JsError {
            kind: $kind,
            message,
        })
    }};
}

pub fn run(inbox: &Receiver<Request>) {
    let mut isolate = v8::Isolate::new(v8::CreateParams::default());
    let context = {
        v8::scope!(let scope, &mut isolate);
        let context = v8::Context::new(scope, v8::ContextOptions::default());
        v8::Global::new(scope, context)
    };
    while let Ok(request) = inbox.recv() {
        match request {
            Request::Eval { source, reply } => {
                let result = {
                    v8::scope_with_context!(let scope, &mut isolate, &context);
                    eval(scope, &source)
                };
                let _ = reply.send(result);
            }
        }
    }
    drop(context);
}

fn eval(scope: &mut v8::PinScope<'_, '_>, source: &str) -> Result<Vec<u8>, EngineError> {
    v8::tc_scope!(let tc, scope);
    let context = tc.get_current_context();
    let source = v8::String::new(tc, source)
        .ok_or_else(|| EngineError::Internal("source too large".to_owned()))?;
    let script =
        v8::Script::compile(tc, source, None).ok_or_else(|| js_error!(tc, JsErrorKind::Parse))?;
    let value = script
        .run(tc)
        .ok_or_else(|| js_error!(tc, JsErrorKind::Runtime))?;
    let serializer = v8::ValueSerializer::new(tc, Box::new(Serializer));
    serializer.write_header();
    match serializer.write_value(context, value) {
        Some(true) => Ok(serializer.release()),
        _ => Err(js_error!(tc, JsErrorKind::Runtime)),
    }
}
