use magnus::scan_args::scan_args;
use magnus::value::ReprValue;
use magnus::{Error, Module, Object, RArray, RHash, RModule, RString, Ruby, TryConvert, Value};
use mini_racer_engine::process::platform;

use crate::{errors, export, rb_raw};

pub fn init(ruby: &Ruby, racer: RModule) -> Result<(), Error> {
    let class = racer.define_class("Platform", ruby.class_object())?;
    class.define_singleton_method("set_flags!", export!(function set_flags, -1))?;
    Ok(())
}

fn set_flags(ruby: &Ruby, args: &[Value]) -> Result<(), Error> {
    let args = scan_args::<(), (), RArray, (), RHash, ()>(args)?;
    for name in args.splat {
        push(ruby, name, None)?;
    }
    // Each `to_s` can collect and compact, so the pairs wait in a Ruby array,
    // which the GC sees and updates, rather than in Rust storage.
    let pairs = ruby.ary_new_capa(args.keywords.len().saturating_mul(2));
    args.keywords.foreach(|name: Value, value: Value| {
        pairs.push(name)?;
        pairs.push(value)?;
        Ok(magnus::r_hash::ForEach::Continue)
    })?;
    while !pairs.is_empty() {
        let name: Value = pairs.shift()?;
        let value: Value = pairs.shift()?;
        push(ruby, name, Some(value))?;
    }
    Ok(())
}

fn to_s(ruby: &Ruby, value: Value) -> Result<Vec<u8>, Error> {
    let string = RString::try_convert(value.funcall::<_, _, Value>("to_s", ())?)?;
    Ok(rb_raw::string_bytes(ruby, string))
}

fn push(ruby: &Ruby, name: Value, value: Option<Value>) -> Result<(), Error> {
    let name = to_s(ruby, name)?;
    let value = match value {
        Some(value) if !value.is_nil() => Some(to_s(ruby, value)?),
        _ => None,
    };
    platform::push_flag(&name, value.as_deref()).map_err(|e| errors::flag(ruby, e))
}
