mod context;
mod decode;
mod errors;
mod gc_root;
mod guard;
mod platform;
#[allow(
    unsafe_code,
    reason = "the only module that calls rb-sys directly (invariant I6)"
)]
mod rb_raw;

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
    errors::init(ruby)?;
    platform::init(ruby, racer)?;
    context::init(ruby, racer)?;
    Ok(())
}
