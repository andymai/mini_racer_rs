use mini_racer_sync::TerminationCause;

use crate::control::JsText;
use crate::serialize::{self, Tc};
use crate::{EngineError, JsError};

const UNEXPECTED: &str = "unexpected failure";

// mini_racer reads the code back from V8 as UTF-8, so a lone surrogate or an
// invalid byte arrives as U+FFFD.
fn lossy(text: &JsText) -> String {
    match text {
        JsText::Utf8(bytes) => String::from_utf8_lossy(bytes).into_owned(),
        JsText::Latin1(bytes) => bytes.iter().copied().map(char::from).collect(),
        JsText::Utf16(units) => char::decode_utf16(units.iter().copied())
            .map(|unit| unit.unwrap_or(char::REPLACEMENT_CHARACTER))
            .collect(),
    }
}

/// Builds a startup snapshot in an isolate of its own, as `mini_racer` does:
/// from `code` run in a fresh context, or, with `base`, by running `code` in
/// a context of that snapshot that is then discarded, keeping the code V8
/// compiled meanwhile.
pub fn create(code: &JsText, base: Option<Vec<u8>>) -> Result<Vec<u8>, EngineError> {
    let warmup = base.is_some();
    let mut isolate = base.map_or_else(
        || v8::Isolate::snapshot_creator(None, None),
        |blob| {
            v8::Isolate::snapshot_creator_from_existing_snapshot(
                v8::StartupData::from(blob),
                None,
                None,
            )
        },
    );
    let (name, handling) = if warmup {
        ("<warmup>", v8::FunctionCodeHandling::Keep)
    } else {
        ("<snapshot>", v8::FunctionCodeHandling::Clear)
    };
    let outcome = {
        v8::scope!(let scope, &mut isolate);
        let context = v8::Context::new(scope, v8::ContextOptions::default());
        let outcome = {
            let scope = &mut v8::ContextScope::new(scope, context);
            v8::tc_scope!(let tc, scope);
            run(tc, &lossy(code), name)
        };
        // V8 needs a default context to write a blob even on the error path.
        let default = if warmup && outcome.is_ok() {
            v8::Context::new(scope, v8::ContextOptions::default())
        } else {
            context
        };
        scope.set_default_context(default);
        outcome
    };
    let blob = isolate.create_blob(handling);
    outcome?;
    blob.map(|blob| blob.to_vec())
        .ok_or_else(|| EngineError::Snapshot(UNEXPECTED.to_owned()))
}

fn run(tc: &Tc<'_, '_, '_, '_>, code: &str, name: &str) -> Result<(), EngineError> {
    let source = v8::String::new(tc, code).ok_or_else(|| failure(tc))?;
    let name = v8::String::new(tc, name).ok_or_else(|| failure(tc))?;
    let origin = serialize::origin(tc, name.into());
    let script = v8::Script::compile(tc, source, Some(&origin)).ok_or_else(|| failure(tc))?;
    script.run(tc).ok_or_else(|| failure(tc))?;
    Ok(())
}

// mini_racer's format: the message, then where it happened.
fn failure(tc: &Tc<'_, '_, '_, '_>) -> EngineError {
    if tc.has_terminated() {
        return EngineError::from(JsError::terminated(TerminationCause::Stop));
    }
    let Some(message) = tc.message() else {
        return EngineError::Snapshot(UNEXPECTED.to_owned());
    };
    let text = message.get(tc).to_rust_string_lossy(tc);
    let Some(name) = message
        .get_script_resource_name(tc)
        .and_then(|name| name.to_string(tc))
    else {
        return EngineError::Snapshot(UNEXPECTED.to_owned());
    };
    let name = name.to_rust_string_lossy(tc);
    let line = message.get_line_number(tc).unwrap_or(0);
    let column = message.get_start_column().cast_signed();
    EngineError::Snapshot(format!("{text}\n{name}:{line}:{column}"))
}
