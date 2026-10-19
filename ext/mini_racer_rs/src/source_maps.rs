use magnus::encoding::{EncodingCapable, RbEncoding};
use magnus::{Error, Object, RModule, RString, Ruby};
use mini_racer_engine::process::{fork, source_maps};

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
    source_maps::check_length(map.len()).map_err(|error| argument(error.to_string()))?;
    let source = String::from_utf8(rb_raw::string_bytes(ruby, source))
        .map_err(|_| argument("the bundle is not valid UTF-8".to_owned()))?;
    let map = rb_raw::string_bytes(ruby, map);
    // The generation is looked up before each try, as an interrupt run
    // between them may run a trap that forks.
    for _ in 0..2 {
        let maps = fork::current().source_maps();
        match rb_raw::nogvl(ruby, || maps.register(&source, &map), &|| {}) {
            Some(Ok(registered)) => return registered.map_err(|error| argument(error.to_string())),
            Some(Err(_)) => return Err(errors::internal(ruby, "source map registration panicked")),
            // An interrupt was pending: run it, then try again.
            None => rb_raw::check_ints(ruby)?,
        }
    }
    // Still one pending that Ruby will not run now, as while it raises.
    fork::current()
        .source_maps()
        .register(&source, &map)
        .map_err(|error| argument(error.to_string()))
}
