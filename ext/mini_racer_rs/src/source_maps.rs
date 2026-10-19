use magnus::encoding::{EncodingCapable, RbEncoding};
use magnus::{Error, Object, RModule, RString, Ruby};
use mini_racer_engine::process::fork;

use crate::{errors, export, rb_raw};

pub fn init(rs: RModule) -> Result<(), Error> {
    rs.define_singleton_method("register_source_map", export!(function register, 2))?;
    Ok(())
}

// Parsed without the GVL, from copies of both Strings, so Ruby's other
// threads run meanwhile and nothing they do can change what is read.
fn register(ruby: &Ruby, source: RString, map: RString) -> Result<(), Error> {
    let argument = |message: String| Error::new(ruby.exception_arg_error(), message);
    // `Context#eval` sends these as Latin-1 and UTF-16, which no bundle
    // matches.
    let encoding = RbEncoding::from(source.enc_get());
    if let name @ ("ISO-8859-1" | "UTF-16LE") = encoding.name() {
        return Err(argument(format!("the bundle must be UTF-8, not {name}")));
    }
    let source = String::from_utf8(rb_raw::string_bytes(ruby, source))
        .map_err(|_| argument("the bundle is not valid UTF-8".to_owned()))?;
    let map = rb_raw::string_bytes(ruby, map);
    let maps = fork::current().source_maps();
    loop {
        match rb_raw::nogvl(ruby, || maps.register(&source, &map), &|| {}) {
            Some(Ok(registered)) => return registered.map_err(|error| argument(error.to_string())),
            Some(Err(_)) => return Err(errors::internal(ruby, "source map registration panicked")),
            // An interrupt was already pending: run it, then parse.
            None => rb_raw::check_ints(ruby)?,
        }
    }
}
