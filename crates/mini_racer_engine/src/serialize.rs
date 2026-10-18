use mini_racer_wire::{Writer, marker};

use crate::{snapshot, sys};
use v8::ValueSerializerHelper;

// mini_racer's filter (mini_racer_v8.cc, MIT, Copyright (c) 2016-2019 the
// mini_racer project authors), byte for byte. It runs in a context user code
// never touched, so tampered prototypes cannot change how a value is copied.
const FILTER_SOURCE: &str = r#"
;(function($globalThis) {
    const {Map: $Map, Set: $Set} = $globalThis
    const sentinel = {}
    return function filter(v) {
        if (typeof v === "function")
            return sentinel
        if (typeof v !== "object" || v === null)
            return v
        if (v instanceof $Map) {
            const m = new Map()
            for (let [k, t] of Map.prototype.entries.call(v)) {
                t = filter(t)
                if (t !== sentinel)
                    m.set(k, t)
            }
            return m
        } else if (v instanceof $Set) {
            const s = new Set()
            for (let t of Set.prototype.values.call(v)) {
                t = filter(t)
                if (t !== sentinel)
                    s.add(t)
            }
            return s
        } else {
            const o = Array.isArray(v) ? [] : {}
            const pds = Object.getOwnPropertyDescriptors(v)
            for (const [k, d] of Object.entries(pds)) {
                if (!d.enumerable)
                    continue
                let t = d.value
                if (d.get) {
                    // *not* d.get.call(...), may have been tampered with
                    t = Function.prototype.call.call(d.get, v, k)
                }
                t = filter(t)
                if (t !== sentinel)
                    Object.defineProperty(o, k, {value: t, enumerable: true})
            }
            return o
        }
    }
})
"#;

pub type Tc<'a, 'b, 'c, 'i> = v8::PinnedRef<'a, v8::TryCatch<'b, 'c, v8::HandleScope<'i>>>;

/// The context `mini_racer` copies uncloneable values in, and its filter.
#[derive(Clone)]
pub struct Filter {
    pub context: v8::Global<v8::Context>,
    function: v8::Global<v8::Function>,
    // The format version V8's serializer writes, which a reply written
    // without it carries too.
    version: u32,
}

pub fn origin<'s>(
    scope: &v8::PinScope<'s, '_>,
    name: v8::Local<'s, v8::Value>,
) -> v8::ScriptOrigin<'s> {
    v8::ScriptOrigin::new(scope, name, 0, 0, false, 0, None, false, false, false, None)
}

/// Creates the safe context, then the user context, as `mini_racer` does. The
/// filter's setup reads the user context's `Map` and `Set` once, under a
/// shared security token that is revoked right after.
///
/// On an isolate made from a snapshot, the safe context is the blob's
/// pristine one, so snapshot code cannot reach the filter's built-ins either.
pub fn contexts(
    scope: &mut v8::PinScope<'_, '_, ()>,
    from_snapshot: bool,
) -> Option<(v8::Global<v8::Context>, Filter)> {
    let safe = from_snapshot
        .then(|| {
            v8::Context::from_snapshot(scope, snapshot::PRISTINE, v8::ContextOptions::default())
        })
        .flatten()
        .unwrap_or_else(|| v8::Context::new(scope, v8::ContextOptions::default()));
    let user = v8::Context::new(scope, v8::ContextOptions::default());
    let (function, version) = {
        let scope = &mut v8::ContextScope::new(scope, safe);
        let source = v8::String::new(scope, FILTER_SOURCE)?;
        let name = v8::String::new(scope, "safe_context_script.js")?;
        let origin = origin(scope, name.into());
        let setup = v8::Script::compile(scope, source, Some(&origin))?.run(scope)?;
        let setup = v8::Local::<v8::Function>::try_from(setup).ok()?;
        safe.set_security_token(user.get_security_token(scope));
        let receiver = v8::undefined(scope).into();
        let global = user.global(scope).into();
        let filter = setup.call(scope, receiver, &[global]);
        safe.use_default_security_token();
        let filter = v8::Local::<v8::Function>::try_from(filter?).ok()?;
        (filter, wire_version(scope)?)
    };
    Some((
        v8::Global::new(scope, user),
        Filter {
            context: v8::Global::new(scope, safe),
            function: v8::Global::new(scope, function),
            version,
        },
    ))
}

fn wire_version(scope: &v8::PinScope<'_, '_>) -> Option<u32> {
    let bytes = write(scope, v8::undefined(scope).into())?;
    let (_, header) = bytes.split_first()?;
    let mut version = 0u32;
    for (shift, byte) in (0..32).step_by(7).zip(header) {
        version |= u32::from(byte & 0x7f).checked_shl(shift)?;
        if byte & 0x80 == 0 {
            return Some(version);
        }
    }
    None
}

/// What `mini_racer` does to a result before serializing it: proxies give way
/// to their targets, symbols to their descriptions, functions to the marker
/// its Ruby side reads as `JavaScriptFunction`, and weak collections and
/// iterators to the entries they would show.
pub fn sanitize<'s>(
    scope: &v8::PinScope<'s, '_>,
    mut value: v8::Local<'s, v8::Value>,
) -> v8::Local<'s, v8::Value> {
    while let Ok(proxy) = v8::Local::<v8::Proxy>::try_from(value) {
        value = proxy.get_target(scope);
    }
    if let Ok(symbol) = v8::Local::<v8::Symbol>::try_from(value) {
        return symbol.description(scope);
    }
    if value.is_function() {
        return v8::String::new_from_two_byte(
            scope,
            &marker::JAVASCRIPT_FUNCTION,
            v8::NewStringType::Normal,
        )
        .map_or(value, Into::into);
    }
    if (value.is_weak_map()
        || value.is_weak_set()
        || value.is_map_iterator()
        || value.is_set_iterator())
        && let Ok(object) = v8::Local::<v8::Object>::try_from(value)
        && let (Some(entries), _) = object.preview_entries(scope)
    {
        return entries.into();
    }
    value
}

struct Delegate;

impl Delegate {
    // V8's message when it has no delegate, which mini_racer's errors carry.
    fn refuse(scope: &v8::PinScope<'_, '_>, object: v8::Local<'_, v8::Object>) {
        let message = format!(
            "#<{}> could not be cloned.",
            object.get_constructor_name().to_rust_string_lossy(scope)
        );
        if let Some(message) = v8::String::new(scope, &message) {
            let exception = v8::Exception::error(scope, message);
            scope.throw_exception(exception);
        }
    }
}

impl v8::ValueSerializerImpl for Delegate {
    fn throw_data_clone_error<'s>(
        &self,
        scope: &mut v8::PinScope<'s, '_>,
        message: v8::Local<'s, v8::String>,
    ) {
        let exception = v8::Exception::error(scope, message);
        scope.throw_exception(exception);
    }

    // Without an exception V8 would report success having written nothing.
    fn write_host_object<'s>(
        &self,
        scope: &mut v8::PinScope<'s, '_>,
        object: v8::Local<'s, v8::Object>,
        _serializer: &dyn ValueSerializerHelper,
    ) -> Option<bool> {
        Self::refuse(scope, object);
        None
    }

    fn get_wasm_module_transfer_id(
        &self,
        scope: &mut v8::PinScope<'_, '_>,
        module: v8::Local<v8::WasmModuleObject>,
    ) -> Option<u32> {
        Self::refuse(scope, module.into());
        None
    }
}

// The bytes V8's serializer writes for a primitive, written without it: for
// these values it checks for no termination and runs no script, and the
// microtasks it would run as it returns ran as the script returned. `None` for
// anything else, and for a NaN, whose bits V8 copies unchanged.
fn primitive(
    isolate: &mut v8::Isolate,
    version: u32,
    value: v8::Local<'_, v8::Value>,
) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let mut writer = Writer::versioned(&mut out, version);
    if sys::is_smi(&value) {
        let small = v8::Local::<v8::Int32>::try_from(value).ok()?;
        writer.integer(small.value().into());
    } else if let Ok(number) = v8::Local::<v8::Number>::try_from(value) {
        let number = number.value();
        if number.is_nan() {
            return None;
        }
        writer.number(number);
    } else if let Ok(string) = v8::Local::<v8::String>::try_from(value) {
        let view = v8::ValueView::new(isolate, string);
        match view.data() {
            v8::ValueViewData::OneByte(bytes) => writer.latin1(bytes),
            v8::ValueViewData::TwoByte(units) => writer.utf16_aligned(units),
        }
    } else if value.is_undefined() {
        writer.undefined();
    } else if value.is_null() {
        writer.null();
    } else if value.is_true() {
        writer.bool(true);
    } else if value.is_false() {
        writer.bool(false);
    } else {
        return None;
    }
    Some(out)
}

/// Serializes `value` with no filter retry; `None` leaves an exception pending.
pub fn write(scope: &v8::PinScope<'_, '_>, value: v8::Local<'_, v8::Value>) -> Option<Vec<u8>> {
    let context = scope.get_current_context();
    let serializer = v8::ValueSerializer::new(scope, Box::new(Delegate));
    serializer.write_header();
    match serializer.write_value(context, value) {
        Some(true) => Some(serializer.release()),
        _ => None,
    }
}

/// Serializes `value` as `mini_racer` replies. A value V8 refuses goes through
/// the filter, which runs its getters a second time, and is serialized again.
/// A failure whose message says the value could not be cloned, from either
/// step, becomes `mini_racer`'s `{"error": <exception>}`. `None` leaves the
/// exception in `tc`.
pub fn reply(
    tc: &mut Tc<'_, '_, '_, '_>,
    filter: &Filter,
    value: v8::Local<'_, v8::Value>,
) -> Option<Vec<u8>> {
    // mini_racer serializes in a TryCatch of its own, which is not verbose, and
    // rethrows from it; V8 never reports a rethrown exception.
    tc.set_verbose(false);
    if let Some(bytes) = primitive(tc, filter.version, value) {
        return Some(bytes);
    }
    if let Some(bytes) = write(tc, value) {
        return Some(bytes);
    }
    if !tc.can_continue() {
        return None;
    }
    tc.reset();
    let function = v8::Local::new(tc, &filter.function);
    let context = v8::Local::new(tc, &filter.context);
    let filtered = {
        let scope = &mut v8::ContextScope::new(&mut **tc, context);
        let receiver = v8::undefined(scope).into();
        function.call(scope, receiver, &[value])
    };
    if let Some(bytes) = filtered.and_then(|filtered| write(tc, filtered)) {
        return Some(bytes);
    }
    if !tc.can_continue() {
        return None;
    }
    // mini_racer tells a clone failure from any other by this text.
    let message = tc.exception()?.to_string(tc)?;
    if !message
        .to_rust_string_lossy(tc)
        .contains("could not be cloned")
    {
        return None;
    }
    tc.reset();
    let error = {
        let scope = &mut v8::ContextScope::new(&mut **tc, context);
        let object = v8::Object::new(scope);
        let key = v8::String::new(scope, "error")?;
        object.set(scope, key.into(), message.into())?;
        object
    };
    write(tc, error.into())
}
