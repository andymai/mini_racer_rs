use magnus::scan_args::scan_args;
use magnus::{Class, Error, Module, Object, RHash, RModule, RString, Ruby, Value};
use mini_racer_engine::ContextHandle;

use crate::{decode, errors, export, rb_raw};

#[magnus::wrap(class = "MiniRacer::Context", free_immediately, size)]
pub struct Context {
    handle: ContextHandle,
}

pub fn init(ruby: &Ruby, racer: RModule) -> Result<(), Error> {
    let class = racer.define_class("Context", ruby.class_object())?;
    class.undef_default_alloc_func();
    class.define_singleton_method("new", export!(function Context::new, -1))?;
    class.define_method("eval", export!(method Context::eval, 1))?;
    Ok(())
}

impl Context {
    fn new(ruby: &Ruby, args: &[Value]) -> Result<Self, Error> {
        let args = scan_args::<(), (), (), (), RHash, ()>(args)?;
        if !args.keywords.is_empty() {
            return Err(Error::new(
                ruby.exception_not_imp_error(),
                "Context options are not supported yet",
            ));
        }
        let handle = ContextHandle::spawn().map_err(|e| errors::engine(ruby, e))?;
        Ok(Self { handle })
    }

    fn eval(ruby: &Ruby, rb_self: &Self, source: RString) -> Result<Value, Error> {
        let source = source.to_string()?;
        let result = rb_raw::without_gvl(ruby, || rb_self.handle.eval(source))
            .map_err(|_| errors::internal(ruby, "panic while evaluating".to_owned()))?;
        let bytes = result.map_err(|e| errors::engine(ruby, e))?;
        decode::value(ruby, &bytes)
    }
}
