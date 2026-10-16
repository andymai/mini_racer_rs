use std::cell::Cell;
use std::sync::OnceLock;

use magnus::encoding::{EncodingCapable, RbEncoding};
use magnus::gc::Marker;
use magnus::r_hash::ForEach;
use magnus::scan_args::scan_args;
use magnus::value::ReprValue;
use magnus::{
    Class, DataTypeFunctions, Error, Fixnum, Module, RHash, RModule, RString, Ruby, Symbol,
    TypedData, Value,
};
use mini_racer_engine::{ContextConfig, ContextHandle, EngineError, JsText};

use crate::gc_root::OwnedRoot;
use crate::host::RubyHost;
use crate::options::{self, OptionError, OptionValue};
use crate::{decode, errors, export, rb_raw};

// What Ruby's GC is told a live context holds outside its heap: roughly an
// isolate and its runner thread.
const ISOLATE_BYTES: usize = 2 << 20;
const POLL: std::time::Duration = std::time::Duration::from_millis(1);

#[derive(Default, TypedData)]
#[magnus(class = "MiniRacer::Context", free_immediately, size, mark)]
pub struct Context {
    handle: OnceLock<ContextHandle>,
    // One dispatch at a time per context, as mini_racer's per-context mutex;
    // a Ruby Mutex so a waiting thread or fiber stays interruptible.
    lock: OwnedRoot<magnus::Mutex>,
    accounted: Cell<bool>,
    // Set before the GVL is released to wait for the runner, so a second
    // initialize cannot start while the first waits.
    initializing: Cell<bool>,
}

impl DataTypeFunctions for Context {
    fn mark(&self, marker: &Marker) {
        self.lock.mark(marker);
    }
}

impl Drop for Context {
    fn drop(&mut self) {
        if self.accounted.get() {
            rb_raw::release_memory(ISOLATE_BYTES);
        }
    }
}

pub fn init(ruby: &Ruby, racer: RModule) -> Result<(), Error> {
    let class = racer.define_class("Context", ruby.class_object())?;
    class.define_alloc_func::<Context>();
    class.define_method("initialize", export!(method Context::initialize, -1))?;
    class.define_method("eval", export!(method Context::eval, -1))?;
    class.define_method("stop", export!(method Context::stop, 0))?;
    class.define_method("dispose", export!(method Context::dispose, 0))?;
    Ok(())
}

struct Locked(magnus::Mutex);

// Mutex#lock's refusal to wait inside a signal trap, told apart by its message
// from a ThreadError another thread raised into this one.
fn refused_in_trap(ruby: &Ruby, error: &Error) -> bool {
    error.is_kind_of(ruby.exception_thread_error())
        && error
            .value()
            .and_then(|exception| exception.funcall::<_, _, String>("message", ()).ok())
            .is_some_and(|message| message == "can't be called from trap context")
}

impl Drop for Locked {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

// mini_racer sends ISO-8859-1 and UTF-16LE strings in their own encoding and
// every other string's bytes as UTF-8.
fn js_text(ruby: &Ruby, string: RString) -> Result<JsText, Error> {
    let bytes = rb_raw::string_bytes(ruby, string);
    Ok(match RbEncoding::from(string.enc_get()).name() {
        "ISO-8859-1" => JsText::Latin1(bytes),
        "UTF-16LE" => {
            let (units, rest) = bytes.as_chunks::<2>();
            if !rest.is_empty() {
                return Err(errors::undeserializable(ruby));
            }
            JsText::Utf16(units.iter().map(|unit| u16::from_le_bytes(*unit)).collect())
        }
        _ => JsText::Utf8(bytes),
    })
}

fn option_value(ruby: &Ruby, value: Value) -> OptionValue {
    let other = || {
        if value.is_nil() {
            OptionValue::Nil
        } else if value.as_value().equal(ruby.qfalse()).unwrap_or(false) {
            OptionValue::False
        } else {
            OptionValue::Other(match value.class().inspect().as_str() {
                "TrueClass" => "true".to_owned(),
                name => name.to_owned(),
            })
        }
    };
    Fixnum::from_value(value).map_or_else(other, |n| OptionValue::Fixnum(n.to_i64()))
}

fn config(ruby: &Ruby, keywords: RHash) -> Result<ContextConfig, Error> {
    let mut pairs = Vec::new();
    keywords.foreach(|key: Symbol, value: Value| {
        pairs.push((key.name()?.into_owned(), option_value(ruby, value)));
        Ok(ForEach::Continue)
    })?;
    options::configure(pairs.into_iter()).map_err(|error| match error {
        OptionError::Type(_) => {
            errors::raised(ruby, ruby.exception_type_error(), &error.to_string())
        }
        OptionError::Range(_) => {
            errors::raised(ruby, ruby.exception_arg_error(), &error.to_string())
        }
        OptionError::Keyword(_) => errors::runtime(ruby, &error.to_string()),
        OptionError::Unsupported(_) => {
            errors::raised(ruby, ruby.exception_not_imp_error(), &error.to_string())
        }
    })
}

impl Context {
    fn handle(&self, ruby: &Ruby) -> Result<&ContextHandle, Error> {
        self.handle
            .get()
            .ok_or_else(|| errors::internal(ruby, "context is not initialized"))
    }

    fn mutex(&self, ruby: &Ruby) -> Result<magnus::Mutex, Error> {
        self.lock
            .get(ruby)
            .ok_or_else(|| errors::internal(ruby, "context is not initialized"))
    }

    // trylock first: an uncontended call then works from a signal trap, where
    // Mutex#lock raises.
    fn acquire(&self, ruby: &Ruby) -> Result<Locked, Error> {
        let mutex = self.mutex(ruby)?;
        if !mutex.trylock() {
            rb_raw::mutex_lock(ruby, mutex)?;
        }
        Ok(Locked(mutex))
    }

    fn initialize(ruby: &Ruby, rb_self: &Self, args: &[Value]) -> Result<(), Error> {
        if rb_self.handle.get().is_some() || rb_self.initializing.get() {
            return Err(errors::internal(ruby, "context is already initialized"));
        }
        let args = scan_args::<(), (), (), (), RHash, ()>(args)?;
        let config = config(ruby, args.keywords)?;
        rb_self.initializing.set(true);
        let mut host = RubyHost::new(ruby);
        let spawned = ContextHandle::spawn(config, &mut host).map_err(|e| errors::engine(ruby, e));
        let handle = host.finish(spawned);
        rb_self.initializing.set(false);
        let handle = handle?;
        rb_self.lock.set(ruby, ruby.mutex_new());
        rb_self
            .handle
            .set(handle)
            .map_err(|_| errors::internal(ruby, "context is already initialized"))?;
        rb_raw::adjust_memory(ruby, ISOLATE_BYTES.cast_signed());
        rb_self.accounted.set(true);
        Ok(())
    }

    fn eval(ruby: &Ruby, rb_self: &Self, args: &[Value]) -> Result<Value, Error> {
        let handle = rb_self.handle(ruby)?;
        let args = scan_args::<(Value,), (), (), (), RHash, ()>(args)?;
        let source = rb_raw::expect_string(ruby, args.required.0)?;
        let filename = match args.keywords.get(ruby.to_symbol("filename")) {
            Some(filename) if !filename.is_nil() => {
                js_text(ruby, rb_raw::expect_string(ruby, filename)?)?
            }
            _ => JsText::from("<eval>"),
        };
        let source = js_text(ruby, source)?;
        // Decoding runs Ruby code, so the lock is released first, as in mini_racer.
        let bytes = {
            let _locked = rb_self.acquire(ruby)?;
            let mut host = RubyHost::new(ruby);
            let result = handle
                .eval(filename, source, &mut host)
                .map_err(|e| errors::engine(ruby, e));
            host.finish(result)?
        };
        decode::value(ruby, &bytes)
    }

    fn stop(ruby: &Ruby, rb_self: &Self) -> Result<(), Error> {
        rb_self
            .handle(ruby)?
            .stop()
            .map_err(|e| errors::engine(ruby, e))
    }

    // Terminates a dispatch running on another thread first, so waiting for the
    // context lock then takes only as long as that dispatch needs to unwind.
    // From the thread that holds the lock (a signal trap during eval) it is
    // refused, as mini_racer refuses it, and the running eval is left alone.
    // Once begun, a dispose finishes: an interrupt while it waits for the lock
    // is kept and raised after the runner is gone, as mini_racer's dispose
    // ignores interrupts until it is done.
    fn dispose(ruby: &Ruby, rb_self: &Self) -> Result<(), Error> {
        let handle = rb_self.handle(ruby)?;
        let mutex = rb_self.mutex(ruby)?;
        if mutex.funcall::<_, _, bool>("owned?", ())? {
            return Err(errors::engine(ruby, EngineError::DisposeBusy));
        }
        handle.begin_dispose();
        let mut interrupted = None;
        let mut polling = false;
        while !mutex.trylock() {
            if polling {
                // Released between attempts, so the thread holding the lock
                // can finish its terminated eval and unlock.
                if rb_raw::nogvl(ruby, || std::thread::sleep(POLL), &|| {}).is_none()
                    && let Err(error) = rb_raw::check_ints(ruby)
                {
                    interrupted.get_or_insert(error);
                }
                continue;
            }
            match rb_raw::mutex_lock(ruby, mutex) {
                Ok(()) => break,
                Err(error) if refused_in_trap(ruby, &error) => polling = true,
                Err(error) => {
                    interrupted.get_or_insert(error);
                }
            }
        }
        let _locked = Locked(mutex);
        let mut host = RubyHost::new(ruby);
        let result = handle
            .dispose(&mut host)
            .map_err(|e| errors::engine(ruby, e));
        let disposed = host.finish(result);
        if disposed.is_ok() && rb_self.accounted.replace(false) {
            rb_raw::adjust_memory(ruby, -ISOLATE_BYTES.cast_signed());
        }
        interrupted.map_or(disposed, Err)
    }
}
