use magnus::{Error, Integer, Module, Object, RModule, Ruby, Value};
use mini_racer_engine::process::{self, fork};

use crate::{errors, export, rb_raw};

pub fn init(ruby: &Ruby, rs: RModule) -> Result<(), Error> {
    let hook = rs.define_module("ForkHook")?;
    hook.define_method("_fork", export!(method fork_hook, 0))?;
    let process: RModule = ruby.class_object().const_get("Process")?;
    process.singleton_class()?.prepend_module(hook)?;
    Ok(())
}

// Process._fork, which every fork from Ruby goes through. With a single-threaded
// V8 the runners go idle first, so the child can take over the contexts that
// were; the GVL is released meanwhile so a runner waiting on a callback can
// finish it.
fn fork_hook(ruby: &Ruby, _process: Value) -> Result<Value, Error> {
    let thread = process::thread_token();
    loop {
        match rb_raw::nogvl(ruby, || fork::prepare_fork(thread), &|| {}) {
            Some(Ok(())) => break,
            Some(Err(_)) => {
                fork::after_fork();
                return Err(errors::internal(ruby, "panic while preparing a fork"));
            }
            None => rb_raw::check_ints(ruby)?,
        }
    }
    let forked = rb_raw::call_super(ruby);
    let child = forked
        .as_ref()
        .ok()
        .and_then(|pid| Integer::from_value(*pid))
        .is_some_and(|pid| pid.to_i64().is_ok_and(|pid| pid == 0));
    if !child {
        fork::after_fork();
    }
    forked
}
