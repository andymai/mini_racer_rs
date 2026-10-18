use std::fmt;
use std::sync::Arc;

use crate::control::JsText;
use crate::serialize::{self, Tc};
use crate::{EngineError, InvalidSnapshot, runner, sys};

const UNEXPECTED: &str = "unexpected failure";

/// Where every blob keeps a context no script has run in, for the filter's
/// safe context of a context made from the blob.
pub const PRISTINE: usize = 0;

// How V8 15.2 lays a blob out (`SnapshotImpl` in v8/src/snapshot/snapshot.cc):
// little-endian words for its context count, whether it can be rehashed and
// two checksums, its version string, then the offsets of its sections. The
// read-only section starts at the first 8-byte boundary after the header, and
// each section opens with V8's magic number and its payload's length.
const CONTEXTS: usize = 0;
const REHASHABLE: usize = 4;
const VERSION_END: usize = 16 + 64;
const STARTUP: usize = 80;
const SHARED_HEAP: usize = 84;
const FIRST_CONTEXT: usize = 88;
const WORD: usize = 4;
const ALIGNMENT: usize = 8;
const SECTION_HEADER: usize = 8;
// V8's magic number is 0xC0DE0000 exclusive-ored with a table size under 2^16,
// so this is its top half.
const MAGIC_HIGH: usize = 0xC0DE;

/// A startup snapshot's blob, which every context made from it shares.
///
/// V8 reads the blob for as long as an isolate made from it lives, whenever it
/// makes a context from it or checks a code cache against it. Empty, it stands
/// for no snapshot, as it does in `mini_racer`.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct SnapshotBlob(Arc<[u8]>);

impl SnapshotBlob {
    pub fn new(bytes: &[u8]) -> Result<Self, InvalidSnapshot> {
        Self::check(bytes)?;
        Ok(Self(Arc::from(bytes)))
    }

    /// Refuses a blob V8 would abort on, or read past the end of, before it
    /// reaches the objects inside: one too short for its header, another V8's,
    /// or one whose sections do not fit in it, as a truncated one's do not.
    /// What a section holds is left to V8, so a blob V8 reads despite a damaged
    /// byte still loads, as it does in `mini_racer`.
    pub fn check(bytes: &[u8]) -> Result<(), InvalidSnapshot> {
        if bytes.is_empty() {
            return Ok(());
        }
        // V8 keeps a blob's length in a C int, and aborts unless the blob runs
        // past the header up to its version.
        if bytes.len() <= VERSION_END || i32::try_from(bytes.len()).is_err() {
            return Err(InvalidSnapshot::Corrupt);
        }
        let header = bytes.get(..=VERSION_END).map(<[u8]>::to_vec);
        if !header.is_some_and(|header| v8::StartupData::from(header).is_valid()) {
            return Err(InvalidSnapshot::Version);
        }
        sections_fit(bytes).ok_or(InvalidSnapshot::Corrupt)
    }

    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Debug for SnapshotBlob {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SnapshotBlob")
            .field("len", &self.0.len())
            .finish()
    }
}

fn word(bytes: &[u8], at: usize) -> Option<usize> {
    let word = bytes.get(at..at.checked_add(WORD)?)?;
    usize::try_from(u32::from_le_bytes(word.try_into().ok()?)).ok()
}

// What V8 checks as it finds each section, the rehashability flag included,
// and the payload length it then trusts without a check.
fn sections_fit(bytes: &[u8]) -> Option<()> {
    let contexts = word(bytes, CONTEXTS).filter(|contexts| *contexts > 0)?;
    word(bytes, REHASHABLE).filter(|flag| *flag <= 1)?;
    let header = contexts.checked_mul(WORD)?.checked_add(FIRST_CONTEXT)?;
    let mut start = header.checked_next_multiple_of(ALIGNMENT)?;
    let magic = word(bytes, start).filter(|magic| magic >> 16 == MAGIC_HIGH)?;
    let ends = [STARTUP, SHARED_HEAP]
        .into_iter()
        .chain((0..contexts).map(|index| FIRST_CONTEXT + index * WORD))
        .map(|at| word(bytes, at))
        .chain([Some(bytes.len())]);
    for end in ends {
        let end = end?;
        let section = bytes.get(start..end)?;
        let payload = word(section, WORD)?;
        if word(section, 0)? != magic || payload > section.len().checked_sub(SECTION_HEADER)? {
            return None;
        }
        start = end;
    }
    Some(())
}

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
/// `building` sees the isolate first, so a pending fork can interrupt it.
pub fn create(
    code: &JsText,
    base: Option<&SnapshotBlob>,
    building: impl FnOnce(&mut v8::Isolate),
) -> Result<Vec<u8>, EngineError> {
    let warmup = base.is_some();
    // `base` outlives the isolate, which `create_blob` disposes of.
    let mut isolate = base.map_or_else(
        || v8::Isolate::snapshot_creator(None, None),
        |blob| {
            v8::Isolate::snapshot_creator_from_existing_snapshot(
                sys::startup_data(blob),
                None,
                None,
            )
        },
    );
    building(&mut isolate);
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
