use crate::EngineError;
use crate::control::JsText;
use crate::runner;
use crate::serialize::{self, Tc};

const UNEXPECTED: &str = "unexpected failure";

/// Where every blob keeps a context no script has run in, for the filter's
/// safe context of a context made from the blob.
pub const PRISTINE: usize = 0;

// mini_racer reads the code back from V8 as UTF-8, which turns a lone
// surrogate into U+FFFD; V8 decodes UTF-8 and Latin-1 as mini_racer's does.
fn source<'s>(tc: &Tc<'s, '_, '_, '_>, code: &JsText) -> Option<v8::Local<'s, v8::String>> {
    match code {
        JsText::Utf16(units) => {
            let mut fixed = Vec::with_capacity(units.len());
            for decoded in char::decode_utf16(units.iter().copied()) {
                let mut buffer = [0; 2];
                let unit = decoded.unwrap_or(char::REPLACEMENT_CHARACTER);
                fixed.extend_from_slice(unit.encode_utf16(&mut buffer));
            }
            runner::string(tc, &JsText::Utf16(fixed))
        }
        JsText::Utf8(_) | JsText::Latin1(_) => runner::string(tc, code),
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
        // A new context of a warmup's base blob would carry the snapshot's
        // script, so the base blob's own pristine context is kept.
        let pristine = warmup
            .then(|| v8::Context::from_snapshot(scope, PRISTINE, v8::ContextOptions::default()))
            .flatten()
            .unwrap_or_else(|| v8::Context::new(scope, v8::ContextOptions::default()));
        scope.add_context(pristine);
        let context = v8::Context::new(scope, v8::ContextOptions::default());
        let outcome = {
            let scope = &mut v8::ContextScope::new(scope, context);
            v8::tc_scope!(let tc, scope);
            run(tc, code, name)
        };
        // V8 needs a default context to write a blob even on the error path;
        // a fresh one leaves the failed script's objects unreachable, so
        // writing it cannot meet one V8 refuses to serialize.
        let default = if warmup || outcome.is_err() {
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

fn run(tc: &Tc<'_, '_, '_, '_>, code: &JsText, name: &str) -> Result<(), EngineError> {
    let source = source(tc, code).ok_or_else(|| failure(tc))?;
    let name = v8::String::new(tc, name).ok_or_else(|| failure(tc))?;
    let origin = serialize::origin(tc, name.into());
    let script = v8::Script::compile(tc, source, Some(&origin)).ok_or_else(|| failure(tc))?;
    script.run(tc).ok_or_else(|| failure(tc))?;
    Ok(())
}

// mini_racer's format: the message, then where it happened.
fn failure(tc: &Tc<'_, '_, '_, '_>) -> EngineError {
    EngineError::Snapshot(runner::located(tc, "\n").unwrap_or_else(|| UNEXPECTED.to_owned()))
}
