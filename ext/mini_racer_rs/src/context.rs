use std::sync::OnceLock;
use std::time::Duration;

use magnus::scan_args::scan_args;
use magnus::{Class, Error, Module, RHash, RModule, RString, Ruby, Value};
use mini_racer_engine::{ContextHandle, Pending, WaitOutcome};

use crate::{decode, errors, export, rb_raw};

#[derive(Default)]
#[magnus::wrap(class = "MiniRacer::Context", free_immediately, size)]
pub struct Context {
    handle: OnceLock<ContextHandle>,
}

pub fn init(ruby: &Ruby, racer: RModule) -> Result<(), Error> {
    let class = racer.define_class("Context", ruby.class_object())?;
    class.define_alloc_func::<Context>();
    class.define_method("initialize", export!(method Context::initialize, -1))?;
    class.define_method("eval", export!(method Context::eval, 1))?;
    Ok(())
}

// After a termination the runner replies within V8's interrupt latency; this
// wait ignores further Ruby interrupts so the reply is never left behind.
fn drain(ruby: &Ruby, pending: &Pending) {
    loop {
        let done = match rb_raw::nogvl(
            ruby,
            || pending.wait_for(Duration::from_millis(100)),
            &|| {},
        ) {
            Some(Ok(done)) => done,
            Some(Err(_)) | None => pending.wait_for(Duration::from_millis(1)),
        };
        if done.is_some() {
            return;
        }
    }
}

impl Context {
    fn handle(&self, ruby: &Ruby) -> Result<&ContextHandle, Error> {
        self.handle
            .get()
            .ok_or_else(|| errors::internal(ruby, "context is not initialized".to_owned()))
    }

    fn initialize(ruby: &Ruby, rb_self: &Self, args: &[Value]) -> Result<(), Error> {
        let args = scan_args::<(), (), (), (), RHash, ()>(args)?;
        if !args.keywords.is_empty() {
            return Err(Error::new(
                ruby.exception_not_imp_error(),
                "Context options are not supported yet",
            ));
        }
        let handle = ContextHandle::spawn().map_err(|e| errors::engine(ruby, e))?;
        rb_self
            .handle
            .set(handle)
            .map_err(|_| errors::internal(ruby, "context is already initialized".to_owned()))
    }

    fn eval(ruby: &Ruby, rb_self: &Self, source: RString) -> Result<Value, Error> {
        let handle = rb_self.handle(ruby)?;
        let pending = handle
            .eval(source.to_string()?)
            .map_err(|e| errors::engine(ruby, e))?;
        loop {
            match rb_raw::nogvl(ruby, || pending.wait(), &|| pending.interrupt()) {
                Some(Ok(WaitOutcome::Done(result))) => {
                    let bytes = result.map_err(|e| errors::engine(ruby, e))?;
                    return decode::value(ruby, &bytes);
                }
                Some(Ok(WaitOutcome::Interrupted)) | None => {
                    if let Err(error) = rb_raw::check_ints(ruby) {
                        handle.terminate(&pending);
                        drain(ruby, &pending);
                        return Err(error);
                    }
                }
                Some(Err(_)) => {
                    handle.terminate(&pending);
                    drain(ruby, &pending);
                    return Err(errors::internal(ruby, "panic while waiting".to_owned()));
                }
            }
        }
    }
}
