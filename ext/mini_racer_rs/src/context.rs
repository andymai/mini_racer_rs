use std::cell::Cell;
use std::io::Write;
use std::sync::OnceLock;

use magnus::encoding::{EncodingCapable, RbEncoding};
use magnus::gc::Marker;
use magnus::r_hash::ForEach;
use magnus::scan_args::scan_args;
use magnus::typed_data::Obj;
use magnus::value::{LazyId, ReprValue};
use magnus::{
    Class, DataTypeFunctions, Error, Fixnum, Module, RArray, RHash, RModule, RString, Ruby, Symbol,
    TryConvert, TypedData, Value,
};
use mini_racer_engine::{ContextConfig, ContextHandle, EngineError, JsText};

use crate::encode;
use crate::gc_root::OwnedRoot;
use crate::host::{Callbacks, RubyHost, Waiter};
use crate::options::{self, OptionError, OptionValue};
use crate::{decode, errors, export, rb_raw, snapshot};

// What Ruby's GC is told a live context holds outside its heap: roughly an
// isolate and its runner thread.
const ISOLATE_BYTES: usize = 2 << 20;
const POLL: std::time::Duration = std::time::Duration::from_millis(1);
// mini_racer warns of every fiftieth level of js->ruby->js nesting.
const DEEP_NESTING: u32 = 50;

static OWNED: LazyId = LazyId::new("owned?");

#[derive(Default, TypedData)]
#[magnus(
    class = "MiniRacer::Context",
    name = "mini_racer/context",
    free_immediately,
    size,
    mark
)]
pub struct Context {
    handle: OnceLock<ContextHandle>,
    // One dispatch at a time per context, as mini_racer's per-context mutex;
    // a Ruby Mutex so a waiting thread or fiber stays interruptible.
    lock: OwnedRoot<magnus::Mutex>,
    // What each attached name calls, indexed by the id V8 sends back.
    callables: OwnedRoot<RArray>,
    raised: OwnedRoot<Value>,
    // The IO a fiber scheduler waits on for this context's runner.
    waiter: Waiter,
    accounted: Cell<bool>,
    // Set before the GVL is released to wait for the runner, so a second
    // initialize cannot start while the first waits.
    initializing: Cell<bool>,
}

impl DataTypeFunctions for Context {
    fn mark(&self, marker: &Marker) {
        self.lock.mark(marker);
        self.callables.mark(marker);
        self.raised.mark(marker);
        self.waiter.mark(marker);
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
    class.define_method("eval_await", export!(method Context::eval_await, -1))?;
    class.define_method("call", export!(method Context::call, -1))?;
    class.define_method("call_await", export!(method Context::call_await, -1))?;
    class.define_method(
        "perform_microtask_checkpoint",
        export!(method Context::perform_microtask_checkpoint, 0),
    )?;
    class.define_method(
        "pump_message_loop",
        export!(method Context::pump_message_loop, 0),
    )?;
    class.define_method("heap_stats", export!(method Context::heap_stats, 0))?;
    class.define_method("heap_snapshot", export!(method Context::heap_snapshot, 0))?;
    class.define_method("attach", export!(method Context::attach, 2))?;
    class.define_method(
        "low_memory_notification",
        export!(method Context::low_memory_notification, 0),
    )?;
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
pub fn js_text(ruby: &Ruby, string: RString) -> Result<JsText, Error> {
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

// mini_racer looks for DateTime at each Context.new until it exists.
fn resolve_date_time(ruby: &Ruby) -> Result<(), Error> {
    if encode::DATE_TIME.get(ruby).is_none()
        && ruby
            .class_object()
            .funcall::<_, _, bool>("const_defined?", ("DateTime",))?
    {
        encode::DATE_TIME.set(ruby, ruby.class_object().const_get::<_, Value>("DateTime")?);
    }
    Ok(())
}

pub fn option_value(ruby: &Ruby, value: Value) -> OptionValue {
    let other = || {
        if value.is_nil() {
            OptionValue::Nil
        } else if value.as_value().equal(ruby.qfalse()).unwrap_or(false) {
            OptionValue::False
        } else {
            OptionValue::Other(rb_raw::type_name(ruby, value))
        }
    };
    Fixnum::from_value(value).map_or_else(other, |n| OptionValue::Fixnum(n.to_i64()))
}

// mini_racer's typed data check names a typed object of the wrong kind by
// its struct name.
fn snapshot_value(ruby: &Ruby, value: Value) -> OptionValue {
    if let Some(shared) = snapshot::shared_of(ruby, value) {
        return OptionValue::Snapshot(shared);
    }
    match (
        option_value(ruby, value),
        rb_raw::typed_data_name(ruby, value),
    ) {
        (OptionValue::Other(_), Some(name)) => OptionValue::Other(name),
        (other, _) => other,
    }
}

fn config(ruby: &Ruby, keywords: RHash) -> Result<ContextConfig, Error> {
    let mut pairs = Vec::new();
    keywords.foreach(|key: Symbol, value: Value| {
        let key = key.name()?.into_owned();
        let value = if key == "snapshot" {
            snapshot_value(ruby, value)
        } else {
            option_value(ruby, value)
        };
        pairs.push((key, value));
        Ok(ForEach::Continue)
    })?;
    options::configure(pairs.into_iter()).map_err(|error| option_error(ruby, &error))
}

pub fn option_error(ruby: &Ruby, error: &OptionError) -> Error {
    match error {
        OptionError::Type(_) | OptionError::SnapshotType(_) => {
            errors::raised(ruby, ruby.exception_type_error(), error.to_string())
        }
        OptionError::Range(_) => {
            errors::raised(ruby, ruby.exception_arg_error(), error.to_string())
        }
        OptionError::Keyword(_) => errors::runtime(ruby, error.to_string()),
        OptionError::InvalidSnapshot(invalid) => errors::engine(ruby, (*invalid).into()),
    }
}

/// Whether `value` is a `MiniRacer::Context` that can no longer run anything.
/// Makes no Ruby call.
pub fn is_disposed(ruby: &Ruby, value: Value) -> bool {
    // Matched by name first, since a failed conversion builds its message
    // through Ruby calls.
    rb_raw::typed_data_name(ruby, value).as_deref() == Some("mini_racer/context")
        && Obj::<Context>::try_convert(value)
            .is_ok_and(|context| context.handle.get().is_none_or(ContextHandle::is_disposed))
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

    fn callables(&self, ruby: &Ruby) -> Result<RArray, Error> {
        self.callables
            .get(ruby)
            .ok_or_else(|| errors::internal(ruby, "context is not initialized"))
    }

    // trylock first: an uncontended call then works from a signal trap, where
    // Mutex#lock raises. A callback's own calls into this context run inside
    // the dispatch that made the callback, which already holds the lock.
    fn acquire(&self, ruby: &Ruby) -> Result<Option<Locked>, Error> {
        let mutex = self.mutex(ruby)?;
        if mutex.trylock() {
            return Ok(Some(Locked(mutex)));
        }
        if self.handle(ruby)?.in_callback() && mutex.funcall::<_, _, bool>(*OWNED, ())? {
            return Ok(None);
        }
        rb_raw::mutex_lock(ruby, mutex)?;
        Ok(Some(Locked(mutex)))
    }

    fn initialize(ruby: &Ruby, rb_self: &Self, args: &[Value]) -> Result<(), Error> {
        resolve_date_time(ruby)?;
        if rb_self.handle.get().is_some() || rb_self.initializing.get() {
            return Err(errors::internal(ruby, "context is already initialized"));
        }
        let args = scan_args::<(), (), (), (), RHash, ()>(args)?;
        let config = config(ruby, args.keywords)?;
        rb_self.initializing.set(true);
        let mut host = RubyHost::new(ruby).waiting_on(&rb_self.waiter);
        let spawned = ContextHandle::spawn(config, &mut host);
        let handle = host.finish(spawned);
        rb_self.initializing.set(false);
        let handle = handle?;
        rb_self.lock.set(ruby, ruby.mutex_new());
        rb_self.callables.set(ruby, ruby.ary_new());
        rb_self
            .handle
            .set(handle)
            .map_err(|_| errors::internal(ruby, "context is already initialized"))?;
        rb_raw::adjust_memory(ruby, ISOLATE_BYTES.cast_signed());
        rb_self.accounted.set(true);
        Ok(())
    }

    fn eval(ruby: &Ruby, rb_self: &Self, args: &[Value]) -> Result<Value, Error> {
        Self::evaluate(ruby, rb_self, args, false)
    }

    fn eval_await(ruby: &Ruby, rb_self: &Self, args: &[Value]) -> Result<Value, Error> {
        Self::evaluate(ruby, rb_self, args, true)
    }

    fn evaluate(
        ruby: &Ruby,
        rb_self: &Self,
        args: &[Value],
        awaited: bool,
    ) -> Result<Value, Error> {
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
        let bytes = rb_self.dispatch(ruby, |host| {
            if awaited {
                handle.eval_await(filename, source, host)
            } else {
                handle.eval(filename, source, host)
            }
        })?;
        decode::value(ruby, &bytes)
    }

    fn call(ruby: &Ruby, rb_self: &Self, args: &[Value]) -> Result<Value, Error> {
        Self::invoke(ruby, rb_self, args, false)
    }

    fn call_await(ruby: &Ruby, rb_self: &Self, args: &[Value]) -> Result<Value, Error> {
        Self::invoke(ruby, rb_self, args, true)
    }

    fn invoke(ruby: &Ruby, rb_self: &Self, args: &[Value], awaited: bool) -> Result<Value, Error> {
        let handle = rb_self.handle(ruby)?;
        let Some((&name, args)) = args.split_first() else {
            // The arity error mini_racer's rb_scan_args raises.
            scan_args::<(Value,), (), RArray, (), (), ()>(args)?;
            return Err(errors::internal(ruby, "no function name"));
        };
        let name = rb_raw::expect_string(ruby, name)?;
        let request = encode::call(ruby, name, args, |message| {
            errors::runtime(ruby, [b"Context.call: ".as_slice(), &message].concat())
        })?;
        let bytes = rb_self.dispatch(ruby, |host| {
            if awaited {
                handle.call_await(request, host)
            } else {
                handle.call(request, host)
            }
        })?;
        decode::value(ruby, &bytes)
    }

    fn perform_microtask_checkpoint(ruby: &Ruby, rb_self: &Self) -> Result<(), Error> {
        let handle = rb_self.handle(ruby)?;
        rb_self.dispatch(ruby, |host| handle.perform_microtask_checkpoint(host))
    }

    fn pump_message_loop(ruby: &Ruby, rb_self: &Self) -> Result<bool, Error> {
        let handle = rb_self.handle(ruby)?;
        rb_self.dispatch(ruby, |host| handle.pump_message_loop(host))
    }

    // mini_racer turns each statistic's name into a symbol.
    fn heap_stats(ruby: &Ruby, rb_self: &Self) -> Result<RHash, Error> {
        let handle = rb_self.handle(ruby)?;
        let bytes = rb_self.dispatch(ruby, |host| handle.heap_stats(host))?;
        let named = RHash::from_value(decode::value(ruby, &bytes)?)
            .ok_or_else(|| errors::internal(ruby, "heap statistics are not an object"))?;
        let stats = ruby.hash_new();
        named.foreach(|name: RString, value: Value| {
            stats.aset(ruby.to_symbol(name.to_string()?), value)?;
            Ok(ForEach::Continue)
        })?;
        Ok(stats)
    }

    // mini_racer sends this one without its result decoding, which also
    // leaves the exception a callback raised for the enclosing dispatch.
    fn heap_snapshot(ruby: &Ruby, rb_self: &Self) -> Result<RString, Error> {
        let handle = rb_self.handle(ruby)?;
        let _locked = rb_self.acquire(ruby)?;
        rb_self.warn_if_deep(ruby)?;
        let mut host = RubyHost::new(ruby).waiting_on(&rb_self.waiter);
        let result = handle.heap_snapshot(&mut host);
        let json = host.finish(result)?;
        Ok(ruby.enc_str_new(json, ruby.utf8_encoding()))
    }

    // The lock is held from choosing the id to sending it, so two threads
    // attaching at once cannot be given the same one.
    fn attach(ruby: &Ruby, rb_self: &Self, name: Value, callable: Value) -> Result<Value, Error> {
        let handle = rb_self.handle(ruby)?;
        let callables = rb_self.callables(ruby)?;
        let name = rb_raw::expect_string(ruby, name)?;
        if handle.is_disposed() {
            return Err(errors::engine(ruby, EngineError::Disposed));
        }
        let _locked = rb_self.acquire(ruby)?;
        let id = i32::try_from(callables.len())
            .map_err(|_| errors::runtime(ruby, "too many callbacks"))?;
        let request = ruby.ary_new_capa(2);
        request.push(name)?;
        request.push(id)?;
        let request = encode::encode(ruby, request.as_value(), |message| {
            errors::runtime(ruby, message)
        })?;
        callables.push(callable)?;
        rb_self.run_locked(ruby, |host| handle.attach(request, host))?;
        Ok(ruby.qnil().as_value())
    }

    // Returns the result undecoded: decoding runs Ruby code, so it happens
    // after the lock is released, as in mini_racer.
    fn dispatch<T>(
        &self,
        ruby: &Ruby,
        run: impl FnOnce(&mut RubyHost<'_>) -> Result<T, EngineError>,
    ) -> Result<T, Error> {
        let _locked = self.acquire(ruby)?;
        self.run_locked(ruby, run)
    }

    fn run_locked<T>(
        &self,
        ruby: &Ruby,
        run: impl FnOnce(&mut RubyHost<'_>) -> Result<T, EngineError>,
    ) -> Result<T, Error> {
        self.warn_if_deep(ruby)?;
        let callbacks = Callbacks {
            callables: self.callables(ruby)?,
            raised: &self.raised,
        };
        let mut host = RubyHost::with_callbacks(ruby, callbacks).waiting_on(&self.waiter);
        let result = run(&mut host);
        host.finish(result)
    }

    fn warn_if_deep(&self, ruby: &Ruby) -> Result<(), Error> {
        let depth = self.handle(ruby)?.depth();
        if depth > 0 && depth % DEEP_NESTING == 0 {
            let _ = writeln!(
                std::io::stderr(),
                "mini_racer: deep js->ruby->js recursion, depth={depth}"
            );
        }
        Ok(())
    }

    // mini_racer sends this one without its result decoding, which also
    // leaves the exception a callback raised for the enclosing dispatch.
    fn low_memory_notification(ruby: &Ruby, rb_self: &Self) -> Result<(), Error> {
        let handle = rb_self.handle(ruby)?;
        let _locked = rb_self.acquire(ruby)?;
        rb_self.warn_if_deep(ruby)?;
        let mut host = RubyHost::new(ruby).waiting_on(&rb_self.waiter);
        let result = handle.low_memory_notification(&mut host);
        host.finish(result)
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
        // From inside a callback, on any fiber of its thread, the lock can
        // never be had, so mini_racer's busy error comes before anything stops.
        if handle.in_callback() || mutex.funcall::<_, _, bool>(*OWNED, ())? {
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
                // Waiting on a lock another fiber of this thread holds.
                Err(error) if error.is_kind_of(ruby.exception_thread_error()) => return Err(error),
                Err(error) => {
                    interrupted.get_or_insert(error);
                }
            }
        }
        let _locked = Locked(mutex);
        let mut host = RubyHost::new(ruby).waiting_on(&rb_self.waiter);
        let result = handle.dispose(&mut host);
        let disposed = host.finish(result);
        if disposed.is_ok() {
            if rb_self.accounted.replace(false) {
                rb_raw::adjust_memory(ruby, -ISOLATE_BYTES.cast_signed());
            }
            if let Some(io) = rb_self.waiter.take(ruby) {
                let _: Value = io.funcall("close", ())?;
            }
        }
        interrupted.map_or(disposed, Err)
    }
}
