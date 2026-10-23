use std::fmt;
use std::sync::{Arc, Weak};

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
// startup section starts at the first 8-byte boundary after the header, and
// each section opens with V8's magic number and its payload's length.
const CONTEXTS: usize = 0;
const REHASHABLE: usize = 4;
const CHECKSUM: usize = 8;
// The checksum covers everything after its own word.
const CHECKSUMMED: usize = 12;
const VERSION_END: usize = 16 + 64;
const READ_ONLY: usize = 80;
const SHARED_HEAP: usize = 84;
const FIRST_CONTEXT: usize = 88;
const WORD: usize = 4;
const ALIGNMENT: usize = 8;
// V8's magic number is 0xC0DE0000 exclusive-ored with a table size under 2^16,
// so this is its top half.
const MAGIC_HIGH: usize = 0xC0DE;
// V8 checksums a blob with zlib's Adler-32 when built with zlib, as
// libv8-node's is, and with its own Fletcher-32 otherwise, as the v8 crate's
// is; mini_racer accepts either.
const ADLER: u64 = 65_521;
const FLETCHER: u64 = 65_535;
// Bytes summed before the sums are reduced; their u64s cannot overflow.
const CHECKSUM_RUN: usize = 1 << 16;

// mini_racer runs this before a `Snapshot.new` script, word for word, since
// the script can read the accessor back, and refuses the script once it has
// read or written `Intl`: V8 cannot write Intl's objects into a blob and aborts
// the process on one.
const INTL_GUARD: &str = r#"
;(function() {
    const original = globalThis.Intl
    let used = false
    const getter = function() {
        used = true
        return original
    }
    const setter = function(value) {
        used = true
    }
    if (!original) {
        const query = function() { return used }
        query.getter = getter
        return query
    }
    Object.defineProperty(globalThis, "Intl", {
        configurable: true,
        enumerable: false,
        get: getter,
        set: setter
    })
    const query = function() { return used }
    query.getter = getter
    return query
})()
"#;
const INTL_REFUSED: &str = "Intl is not supported in Snapshot.new scripts";

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

    /// Refuses a blob as `mini_racer` does, in its order: one too long for
    /// V8, too short for its header, another V8's, one whose header or section
    /// offsets do not fit it, or whose checksum does not match. Then, past
    /// those, a blob with a section V8 aborts the process on, which
    /// `mini_racer` hands V8.
    pub fn check(bytes: &[u8]) -> Result<(), InvalidSnapshot> {
        if bytes.is_empty() {
            return Ok(());
        }
        if i32::try_from(bytes.len()).is_err() {
            return Err(InvalidSnapshot::TooLarge);
        }
        if bytes.len() < FIRST_CONTEXT {
            return Err(InvalidSnapshot::TooShort);
        }
        let header = bytes.get(..=VERSION_END).map(<[u8]>::to_vec);
        if !header.is_some_and(|header| v8::StartupData::from(header).is_valid()) {
            return Err(InvalidSnapshot::Version);
        }
        let ends = section_ends(bytes)?;
        if !checksum_matches(bytes) {
            return Err(InvalidSnapshot::Checksum);
        }
        sections_open(bytes, &ends)
    }

    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    #[must_use]
    pub fn downgrade(&self) -> WeakSnapshotBlob {
        WeakSnapshotBlob(Arc::downgrade(&self.0))
    }
}

/// A `SnapshotBlob` that lives only while a context made from it does.
#[derive(Debug, Clone)]
pub struct WeakSnapshotBlob(Weak<[u8]>);

impl WeakSnapshotBlob {
    #[must_use]
    pub fn upgrade(&self) -> Option<SnapshotBlob> {
        self.0.upgrade().map(SnapshotBlob)
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

// Where each section starts, from the startup section to the end of the blob,
// with `mini_racer`'s checks on the header and the offsets in its order.
fn section_ends(bytes: &[u8]) -> Result<Vec<usize>, InvalidSnapshot> {
    let len = bytes.len();
    let read = |at| word(bytes, at).ok_or(InvalidSnapshot::TooShort);
    let contexts = read(CONTEXTS)?;
    if read(REHASHABLE)? > 1 {
        return Err(InvalidSnapshot::Header);
    }
    if contexts == 0 {
        return Err(InvalidSnapshot::NoContexts);
    }
    if contexts > (len - FIRST_CONTEXT) / WORD {
        return Err(InvalidSnapshot::ContextTable);
    }
    let startup = (FIRST_CONTEXT + contexts * WORD).next_multiple_of(ALIGNMENT);
    if startup > len {
        return Err(InvalidSnapshot::ContextTable);
    }
    let read_only = read(READ_ONLY)?;
    if read_only <= startup || read_only > len {
        return Err(InvalidSnapshot::ReadOnly);
    }
    let shared_heap = read(SHARED_HEAP)?;
    if shared_heap <= read_only || shared_heap > len {
        return Err(InvalidSnapshot::SharedHeap);
    }
    let mut ends = vec![startup, read_only, shared_heap];
    for index in 0..contexts {
        let offset = read(FIRST_CONTEXT + index * WORD)?;
        if ends.last().is_some_and(|previous| offset <= *previous) || offset >= len {
            return Err(InvalidSnapshot::Context);
        }
        ends.push(offset);
    }
    ends.push(len);
    Ok(ends)
}

fn checksum_matches(bytes: &[u8]) -> bool {
    let checksummed = bytes.get(CHECKSUMMED..).unwrap_or_default();
    let expected = word(bytes, CHECKSUM).and_then(|word| u64::try_from(word).ok());
    [ADLER, FLETCHER]
        .into_iter()
        .any(|modulus| Some(checksum(checksummed, modulus)) == expected)
}

// V8's `Checksum`: the two running sums from zero, each reduced by `modulus`.
fn checksum(bytes: &[u8], modulus: u64) -> u64 {
    let (mut low, mut high) = (0_u64, 0_u64);
    for run in bytes.chunks(CHECKSUM_RUN) {
        for &byte in run {
            low += u64::from(byte);
            high += low;
        }
        low %= modulus;
        high %= modulus;
    }
    high << 16 | low
}

// V8 aborts on a section that does not open with its magic number, which every
// section shares, or whose payload's length does not fit a C int. `ends` starts
// at the startup section, so each section starts at one of its offsets but the
// last, in the order of mini_racer's messages.
fn sections_open(bytes: &[u8], ends: &[usize]) -> Result<(), InvalidSnapshot> {
    let magic = ends
        .first()
        .and_then(|start| word(bytes, *start))
        .filter(|magic| magic >> 16 == MAGIC_HIGH)
        .ok_or(InvalidSnapshot::Startup)?;
    let starts = ends.split_last().map_or(&[][..], |(_, starts)| starts);
    for (index, start) in starts.iter().enumerate() {
        let payload = start.checked_add(WORD).and_then(|at| word(bytes, at));
        if word(bytes, *start) != Some(magic)
            || payload.is_none_or(|payload| i32::try_from(payload).is_err())
        {
            return Err(match index {
                0 => InvalidSnapshot::Startup,
                1 => InvalidSnapshot::ReadOnly,
                2 => InvalidSnapshot::SharedHeap,
                _ => InvalidSnapshot::Context,
            });
        }
    }
    Ok(())
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
/// compiled meanwhile. An empty `base` warms up V8's own snapshot.
/// `building` sees the isolate first, so a pending fork can interrupt it.
pub fn create(
    code: &JsText,
    base: Option<&SnapshotBlob>,
    building: impl FnOnce(&mut v8::Isolate),
) -> Result<Vec<u8>, EngineError> {
    let warmup = base.is_some();
    let base = base.filter(|blob| !blob.is_empty());
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
        let pristine = base
            .and_then(|_| {
                v8::Context::from_snapshot(scope, PRISTINE, v8::ContextOptions::default())
            })
            .unwrap_or_else(|| v8::Context::new(scope, v8::ContextOptions::default()));
        scope.add_context(pristine);
        let context = v8::Context::new(scope, v8::ContextOptions::default());
        let outcome = {
            let scope = &mut v8::ContextScope::new(scope, context);
            v8::tc_scope!(let tc, scope);
            if warmup {
                run(tc, code, name)
            } else {
                run_guarded(tc, code, name)
            }
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

fn run_guarded(tc: &Tc<'_, '_, '_, '_>, code: &JsText, name: &str) -> Result<(), EngineError> {
    let guard = guard_intl(tc)?;
    run(tc, code, name)?;
    if let Some(guard) = guard {
        guard.release(tc)?;
    }
    Ok(())
}

struct IntlGuard<'s> {
    original: v8::Local<'s, v8::Value>,
    used: v8::Local<'s, v8::Function>,
    getter: v8::Local<'s, v8::Function>,
}

fn internal(message: &str) -> EngineError {
    EngineError::Snapshot(message.to_owned())
}

fn intl_key<'s>(tc: &Tc<'s, '_, '_, '_>) -> Option<v8::Local<'s, v8::Name>> {
    v8::String::new(tc, "Intl").map(Into::into)
}

// None when the global has no `Intl` object to guard, as in mini_racer.
fn guard_intl<'s>(tc: &Tc<'s, '_, '_, '_>) -> Result<Option<IntlGuard<'s>>, EngineError> {
    let global = tc.get_current_context().global(tc);
    let original = intl_key(tc)
        .and_then(|key| global.get(tc, key.into()))
        .ok_or_else(|| internal("failed to inspect snapshot Intl"))?;
    if !original.is_object() {
        return Ok(None);
    }
    let failed = || internal("failed to initialize snapshot Intl guard");
    let source = v8::String::new(tc, INTL_GUARD).ok_or_else(failed)?;
    let used = v8::Script::compile(tc, source, None)
        .and_then(|script| script.run(tc))
        .and_then(|used| v8::Local::<v8::Function>::try_from(used).ok())
        .ok_or_else(failed)?;
    let getter = v8::String::new(tc, "getter")
        .and_then(|key| used.get(tc, key.into()))
        .and_then(|getter| v8::Local::<v8::Function>::try_from(getter).ok())
        .ok_or_else(failed)?;
    Ok(Some(IntlGuard {
        original,
        used,
        getter,
    }))
}

impl IntlGuard<'_> {
    // Refuses a script that read or wrote `Intl`, then puts `Intl` back unless
    // the script replaced or deleted the accessor. Each lookup is of an own
    // property, so nothing a script left on `Object.prototype` runs here.
    fn release(self, tc: &Tc<'_, '_, '_, '_>) -> Result<(), EngineError> {
        let undefined = v8::undefined(tc).into();
        let used = self
            .used
            .call(tc, undefined, &[])
            .ok_or_else(|| internal("failed to inspect snapshot Intl guard"))?;
        if used.is_true() {
            return Err(EngineError::Snapshot(INTL_REFUSED.to_owned()));
        }
        let inspect = || internal("failed to inspect Intl after snapshot script");
        let global = tc.get_current_context().global(tc);
        let key = intl_key(tc).ok_or_else(inspect)?;
        let descriptor = global
            .get_own_property_descriptor(tc, key)
            .ok_or_else(inspect)?;
        let mut ours = false;
        if let Ok(descriptor) = v8::Local::<v8::Object>::try_from(descriptor) {
            let get = v8::String::new(tc, "get").ok_or_else(inspect)?;
            let getter = descriptor
                .get_own_property_descriptor(tc, get.into())
                .ok_or_else(inspect)?;
            if let Ok(getter) = v8::Local::<v8::Object>::try_from(getter) {
                let getter = v8::String::new(tc, "value")
                    .and_then(|value| getter.get(tc, value.into()))
                    .ok_or_else(inspect)?;
                ours = getter.is_function() && getter.same_value(self.getter.into());
            }
        }
        if ours
            && !global
                .define_own_property(tc, key, self.original, v8::PropertyAttribute::DONT_ENUM)
                .unwrap_or(false)
        {
            return Err(internal("failed to restore Intl after snapshot script"));
        }
        Ok(())
    }
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
