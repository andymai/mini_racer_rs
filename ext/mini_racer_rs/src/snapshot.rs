use std::cell::RefCell;

use magnus::encoding::EncodingCapable;
use magnus::gc::Marker;
use magnus::scan_args::scan_args;
use magnus::typed_data::Obj;
use magnus::value::ReprValue;
use magnus::{
    Class, DataTypeFunctions, Error, Module, Object, RClass, RModule, RString, Ruby, TryConvert,
    TypedData, Value,
};
use mini_racer_engine::{ContextHandle, InvalidSnapshot, SnapshotBlob, WeakSnapshotBlob};

use crate::gc_root::OwnedRoot;
use crate::host::RubyHost;
use crate::{context, errors, export, rb_raw};

#[derive(Default, TypedData)]
#[magnus(
    class = "MiniRacer::Snapshot",
    name = "mini_racer/snapshot",
    free_immediately,
    mark,
    size
)]
pub struct Snapshot {
    blob: OwnedRoot<RString>,
    // What contexts made from this snapshot start from, reused while one of
    // them lives and `blob` holds the same bytes. Held weakly, so once its
    // contexts are gone a snapshot holds only `blob`, as mini_racer's does.
    shared: RefCell<Option<WeakSnapshotBlob>>,
}

impl DataTypeFunctions for Snapshot {
    fn mark(&self, marker: &Marker) {
        self.blob.mark(marker);
    }

    fn size(&self) -> usize {
        let blob = Ruby::get().ok().and_then(|ruby| self.blob.get(&ruby));
        size_of::<Self>().saturating_add(blob.map_or(0, RString::len))
    }
}

pub fn init(ruby: &Ruby, racer: RModule) -> Result<(), Error> {
    let class = racer.define_class("Snapshot", ruby.class_object())?;
    class.define_alloc_func::<Snapshot>();
    class.define_method("initialize", export!(method Snapshot::initialize, -1))?;
    class.define_method("dump", export!(method Snapshot::dump, 0))?;
    class.define_method("size", export!(method Snapshot::size, 0))?;
    class.define_method("warmup!", export!(method Snapshot::warmup, 1))?;
    class.define_singleton_method("load", export!(method Snapshot::load, 1))?;
    Ok(())
}

/// The blob a context made from `value`, when it is a `MiniRacer::Snapshot`,
/// starts from, for `Context.new`.
pub fn shared_of(ruby: &Ruby, value: Value) -> Option<Result<SnapshotBlob, InvalidSnapshot>> {
    let snapshot = Obj::<Snapshot>::try_convert(value).ok()?;
    Some(snapshot.shared(ruby))
}

// mini_racer raises every failure of a build as a snapshot error, V8's
// refusal of an odd-length UTF-16LE string included, which it meets before
// the base blob.
fn build(ruby: &Ruby, code: RString, base: Option<&Snapshot>) -> Result<RString, Error> {
    let code = context::js_text(ruby, code)
        .map_err(|_| errors::snapshot(ruby, "Error: Unable to deserialize cloned data."))?;
    let base = base
        .map(|base| base.shared(ruby))
        .transpose()
        .map_err(|invalid| errors::engine(ruby, invalid.into()))?;
    let mut host = RubyHost::new(ruby);
    let built = ContextHandle::create_snapshot(code, base, &mut host);
    let blob = host.finish(built)?;
    Ok(ruby.str_from_slice(&blob))
}

impl Snapshot {
    fn shared(&self, ruby: &Ruby) -> Result<SnapshotBlob, InvalidSnapshot> {
        let blob = self.blob(ruby);
        let mut shared = self.shared.borrow_mut();
        let kept = shared.as_ref().and_then(WeakSnapshotBlob::upgrade);
        let current = rb_raw::with_string_bytes(ruby, blob, |bytes| match kept {
            Some(kept) if kept.as_bytes() == bytes => Ok(kept),
            _ => SnapshotBlob::new(bytes),
        })?;
        *shared = Some(current.downgrade());
        Ok(current)
    }

    fn replace(&self, ruby: &Ruby, blob: RString) {
        self.blob.set(ruby, blob);
        self.shared.borrow_mut().take();
    }

    // mini_racer allocates every snapshot with an empty blob.
    fn blob(&self, ruby: &Ruby) -> RString {
        self.blob.get(ruby).unwrap_or_else(|| {
            let empty = ruby.str_from_slice(b"");
            self.blob.set(ruby, empty);
            empty
        })
    }

    fn initialize(ruby: &Ruby, rb_self: &Self, args: &[Value]) -> Result<(), Error> {
        let args = scan_args::<(), (Option<Value>,), (), (), (), ()>(args)?;
        let code = match args.optional.0 {
            Some(code) if !code.is_nil() => rb_raw::expect_string(ruby, code)?,
            _ => ruby.str_new(""),
        };
        rb_self.replace(ruby, build(ruby, code, None)?);
        Ok(())
    }

    // A copy, as mini_racer's is, so changing it changes nothing a context
    // starts from.
    fn dump(ruby: &Ruby, rb_self: &Self) -> RString {
        rb_raw::str_dup(ruby, rb_self.blob(ruby))
    }

    fn size(ruby: &Ruby, rb_self: &Self) -> usize {
        rb_self.blob(ruby).len()
    }

    // The blob is replaced only once the warmed-up one is built, so a failed
    // warmup leaves the snapshot as it was.
    fn warmup(ruby: &Ruby, rb_self: Obj<Self>, code: Value) -> Result<Obj<Self>, Error> {
        let code = rb_raw::expect_string(ruby, code)?;
        let warmed = build(ruby, code, Some(&rb_self))?;
        rb_self.replace(ruby, warmed);
        Ok(rb_self)
    }

    fn load(ruby: &Ruby, class: RClass, blob: Value) -> Result<Obj<Self>, Error> {
        let blob = rb_raw::expect_string(ruby, blob)?;
        let copy = rb_raw::str_dup(ruby, blob);
        copy.enc_associate(ruby.ascii8bit_encindex())?;
        rb_raw::with_string_bytes(ruby, copy, SnapshotBlob::check)
            .map_err(|invalid| errors::engine(ruby, invalid.into()))?;
        let snapshot = ruby.obj_wrap_as(Self::default(), class);
        snapshot.blob.set(ruby, copy);
        Ok(snapshot)
    }
}
