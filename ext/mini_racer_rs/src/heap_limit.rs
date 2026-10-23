use magnus::value::ReprValue;
use magnus::{Error, Integer, Object, RModule, Ruby, Value};
use mini_racer_engine::process::platform;

use crate::{errors, export, rb_raw};

pub fn init(rs: RModule) -> Result<(), Error> {
    rs.define_singleton_method("heap_limit", export!(function limit, 0))?;
    rs.define_singleton_method("heap_limit=", export!(function set_limit, 1))?;
    Ok(())
}

fn limit(ruby: &Ruby) -> Value {
    platform::heap_limit()
        .and_then(|limit| u64::try_from(limit).ok())
        .map_or_else(
            || ruby.qnil().as_value(),
            |limit| ruby.integer_from_u64(limit).as_value(),
        )
}

// Checked as `Context.new` checks its integer options: anything but an
// Integer is a TypeError, and an Integer out of range, however large, an
// ArgumentError.
fn set_limit(ruby: &Ruby, value: Value) -> Result<Value, Error> {
    let limit = if value.is_nil() {
        None
    } else {
        let integer = Integer::from_value(value).ok_or_else(|| {
            let name = rb_raw::type_name(ruby, value);
            errors::raised(
                ruby,
                ruby.exception_type_error(),
                format!("wrong argument type {name} (expected Integer)"),
            )
        })?;
        let bytes = integer.to_i64().ok().and_then(|n| usize::try_from(n).ok());
        Some(bytes.unwrap_or(usize::MAX))
    };
    platform::set_heap_limit(limit).map_err(|error| errors::heap_limit(ruby, error))?;
    Ok(value)
}
