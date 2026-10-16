use std::sync::Arc;

use magnus::gc::Marker;
use magnus::scan_args::scan_args;
use magnus::typed_data::Obj;
use magnus::value::ReprValue;
use magnus::{
    Class, DataTypeFunctions, Error, Module, Object, RModule, RString, Ruby, TryConvert, TypedData,
    Value,
};
use mini_racer_engine::ContextHandle;

use crate::gc_root::OwnedRoot;
use crate::host::RubyHost;
use crate::{context, export, rb_raw};

#[derive(Default, TypedData)]
#[magnus(class = "MiniRacer::Snapshot", free_immediately, mark)]
pub struct Snapshot {
    blob: OwnedRoot<RString>,
}

impl DataTypeFunctions for Snapshot {
    fn mark(&self, marker: &Marker) {
        self.blob.mark(marker);
    }
}

pub fn init(ruby: &Ruby, racer: RModule) -> Result<(), Error> {
    let class = racer.define_class("Snapshot", ruby.class_object())?;
    class.define_alloc_func::<Snapshot>();
    class.define_method("initialize", export!(method Snapshot::initialize, -1))?;
    class.define_method("dump", export!(method Snapshot::dump, 0))?;
    class.define_method("size", export!(method Snapshot::size, 0))?;
    class.define_method("warmup!", export!(method Snapshot::warmup, 1))?;
    class.define_singleton_method("load", export!(function Snapshot::load, 1))?;
    Ok(())
}

/// The blob of `value` when it is a `MiniRacer::Snapshot`, for `Context.new`.
pub fn blob_of(ruby: &Ruby, value: Value) -> Option<Arc<[u8]>> {
    let snapshot = Obj::<Snapshot>::try_convert(value).ok()?;
    Some(Arc::from(snapshot.bytes(ruby)))
}

fn build(ruby: &Ruby, code: RString, base: Option<Vec<u8>>) -> Result<RString, Error> {
    let code = context::js_text(ruby, code)?;
    let mut host = RubyHost::new(ruby);
    let built = ContextHandle::create_snapshot(code, base, &mut host);
    let blob = host.finish(built)?;
    Ok(ruby.str_from_slice(&blob))
}

impl Snapshot {
    fn bytes(&self, ruby: &Ruby) -> Vec<u8> {
        self.blob
            .get(ruby)
            .map(|blob| rb_raw::string_bytes(ruby, blob))
            .unwrap_or_default()
    }

    fn initialize(ruby: &Ruby, rb_self: &Self, args: &[Value]) -> Result<(), Error> {
        let args = scan_args::<(), (Option<Value>,), (), (), (), ()>(args)?;
        let code = match args.optional.0 {
            Some(code) if !code.is_nil() => rb_raw::expect_string(ruby, code)?,
            _ => ruby.str_new(""),
        };
        rb_self.blob.set(ruby, build(ruby, code, None)?);
        Ok(())
    }

    fn dump(ruby: &Ruby, rb_self: &Self) -> RString {
        rb_self
            .blob
            .get(ruby)
            .unwrap_or_else(|| ruby.str_from_slice(b""))
    }

    fn size(ruby: &Ruby, rb_self: &Self) -> usize {
        rb_self.blob.get(ruby).map_or(0, RString::len)
    }

    // The blob is replaced only once the warmed-up one is built, so a failed
    // warmup leaves the snapshot as it was.
    fn warmup(ruby: &Ruby, rb_self: Obj<Self>, code: Value) -> Result<Obj<Self>, Error> {
        let code = rb_raw::expect_string(ruby, code)?;
        let base = rb_self.bytes(ruby);
        let warmed = build(ruby, code, Some(base))?;
        rb_self.blob.set(ruby, warmed);
        Ok(rb_self)
    }

    fn load(ruby: &Ruby, blob: Value) -> Result<Obj<Self>, Error> {
        let blob = rb_raw::expect_string(ruby, blob)?;
        let copy = ruby.str_from_slice(&rb_raw::string_bytes(ruby, blob));
        let snapshot = ruby.obj_wrap(Self::default());
        snapshot.blob.set(ruby, copy);
        Ok(snapshot)
    }
}
