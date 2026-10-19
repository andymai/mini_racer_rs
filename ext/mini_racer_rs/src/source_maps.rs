use magnus::{Error, Object, RModule, RString, Ruby};
use mini_racer_engine::process::fork;

use crate::{export, rb_raw};

pub fn init(rs: RModule) -> Result<(), Error> {
    rs.define_singleton_method("register_source_map", export!(function register, 2))?;
    Ok(())
}

// Parsed under the GVL, from copies of both Strings, so nothing Ruby runs
// meanwhile can change them.
fn register(ruby: &Ruby, source: RString, map: RString) -> Result<(), Error> {
    let argument = |message: String| Error::new(ruby.exception_arg_error(), message);
    let source = String::from_utf8(rb_raw::string_bytes(ruby, source))
        .map_err(|_| argument("the bundle is not valid UTF-8".to_owned()))?;
    let map = rb_raw::string_bytes(ruby, map);
    fork::current()
        .source_maps()
        .register(&source, &map)
        .map_err(|error| argument(error.to_string()))
}
