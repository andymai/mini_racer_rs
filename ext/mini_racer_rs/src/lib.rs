mod context;
mod decode;
mod encode;
mod errors;
mod fork;
#[allow(
    unsafe_code,
    reason = "a decode's spilled values are lent in place as the slice Ruby reads"
)]
mod gc_root;
mod guard;
mod host;
mod options;
mod platform;
#[allow(
    unsafe_code,
    reason = "the only module that calls rb-sys directly (invariant I6)"
)]
mod rb_raw;
mod snapshot;
mod source_maps;

use magnus::{Error, Module, Ruby};

macro_rules! export {
    (method $f:path, $($arity:tt)+) => {
        magnus::method!($f, $($arity)+)
    };
    (function $f:path, $($arity:tt)+) => {
        magnus::function!($f, $($arity)+)
    };
}
pub(crate) use export;

#[magnus::init(name = "mini_racer_rs")]
fn init(ruby: &Ruby) -> Result<(), Error> {
    let racer = ruby.define_module("MiniRacer")?;
    let rs = ruby.define_module("MiniRacerRs")?;
    rs.const_set("NATIVE_VERSION", env!("CARGO_PKG_VERSION"))?;
    rs.const_set("V8_VERSION", mini_racer_engine::process::v8_version())?;
    racer.define_class("JavaScriptFunction", ruby.class_object())?;
    gc_root::init(ruby, rs)?;
    errors::init(ruby)?;
    decode::init(ruby);
    encode::init(ruby);
    platform::init(ruby, racer)?;
    context::init(ruby, racer)?;
    snapshot::init(ruby, racer)?;
    source_maps::init(rs)?;
    fork::init(ruby, rs)?;
    Ok(())
}
