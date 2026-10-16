use magnus::{Error, Module, Object, RModule, Ruby, Value};
use mini_racer_engine::process::fork;

use crate::{export, rb_raw};

pub fn init(ruby: &Ruby, rs: RModule) -> Result<(), Error> {
    let hook = rs.define_module("ForkHook")?;
    hook.define_method("_fork", export!(method fork_hook, 0))?;
    hook.define_method("daemon", export!(method daemon_hook, -1))?;
    let process: RModule = ruby.class_object().const_get("Process")?;
    process.singleton_class()?.prepend_module(hook)?;
    Ok(())
}

// Process._fork, which every fork from Ruby goes through except
// Process.daemon's. The engine's own fork handlers, inside fork() itself, wait
// for the runners only on forks marked this way, whose child runs Ruby.
fn fork_hook(ruby: &Ruby, _process: Value) -> Result<Value, Error> {
    forking(ruby, &[])
}

fn daemon_hook(ruby: &Ruby, _process: Value, args: &[Value]) -> Result<Value, Error> {
    forking(ruby, args)
}

fn forking(ruby: &Ruby, args: &[Value]) -> Result<Value, Error> {
    fork::intend(true);
    let forked = rb_raw::call_super(ruby, args);
    fork::intend(false);
    forked
}
